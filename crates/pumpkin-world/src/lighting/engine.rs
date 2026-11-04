use crate::chunk::format::LightContainer;
use crate::chunk_system::Chunk;
use crate::chunk_system::generation_cache::Cache;
use crate::generation::height_limit::HeightLimitView;
use crate::generation::proto_chunk::GenerationCache;
use crate::lighting::storage::{get_block_light, get_sky_light, set_block_light, set_sky_light};
use pumpkin_config::lighting::LightingEngineConfig;
use pumpkin_data::{BlockDirection, BlockState, BlockStateId};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use std::collections::VecDeque;

pub const MAX_LEVEL: u8 = 15;
const MIN_OPACITY: u8 = 1;
const PROPAGATION_DIRECTIONS: [BlockDirection; 6] = BlockDirection::all();

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum LightLayer {
    Sky,
    Block,
}

/// Blocks and stored light the propagation runs against, like vanilla's `LightChunkGetter`
/// together with the layer's section storage.
pub trait LightStorage {
    /// The stored level, or `None` where no light is stored (vanilla `storingLightForSection`).
    fn get_level(&self, layer: LightLayer, pos: BlockPos) -> Option<u8>;
    fn set_level(&mut self, layer: LightLayer, pos: BlockPos, level: u8);
    fn get_state(&self, pos: BlockPos) -> &'static BlockState;
    /// `ChunkSkyLightSources.getLowestSourceY` of the column, or `default` if its chunk is missing.
    fn lowest_sky_source_y(&self, x: i32, z: i32, default: i32) -> i32;
    fn bottom_y(&self) -> i32;
    /// One above the highest block.
    fn top_y(&self) -> i32;
}

/// Vanilla `LightEngine.QueueEntry`: the level, the directions to visit and two flags packed in a `u16`.
mod queue_entry {
    use pumpkin_data::BlockDirection;

    const LEVEL_MASK: u16 = 15;
    const DIRECTIONS_MASK: u16 = 0b11_1111 << 4;
    const FLAG_FROM_EMPTY_SHAPE: u16 = 1 << 10;
    const FLAG_INCREASE_FROM_EMISSION: u16 = 1 << 11;

    pub const fn decrease_skip_one_direction(old_from_level: u8, skip: BlockDirection) -> u16 {
        with_level(without_direction(DIRECTIONS_MASK, skip), old_from_level)
    }

    pub const fn decrease_all_directions(old_from_level: u8) -> u16 {
        with_level(DIRECTIONS_MASK, old_from_level)
    }

    pub const fn increase_light_from_emission(new_from_level: u8, from_empty_shape: bool) -> u16 {
        let data = DIRECTIONS_MASK | FLAG_INCREASE_FROM_EMISSION;
        with_level(with_empty_shape(data, from_empty_shape), new_from_level)
    }

    pub const fn increase_skip_one_direction(
        new_from_level: u8,
        from_empty_shape: bool,
        skip: BlockDirection,
    ) -> u16 {
        let data = without_direction(DIRECTIONS_MASK, skip);
        with_level(with_empty_shape(data, from_empty_shape), new_from_level)
    }

    pub const fn increase_only_one_direction(
        new_from_level: u8,
        from_empty_shape: bool,
        direction: BlockDirection,
    ) -> u16 {
        let data = with_direction(with_empty_shape(0, from_empty_shape), direction);
        with_level(data, new_from_level)
    }

    #[expect(
        clippy::fn_params_excessive_bools,
        reason = "mirrors vanilla QueueEntry.increaseSkySourceInDirections"
    )]
    pub const fn increase_sky_source_in_directions(
        down: bool,
        north: bool,
        south: bool,
        west: bool,
        east: bool,
    ) -> u16 {
        let mut data = with_level(0, super::MAX_LEVEL);
        if down {
            data = with_direction(data, BlockDirection::Down);
        }
        if north {
            data = with_direction(data, BlockDirection::North);
        }
        if south {
            data = with_direction(data, BlockDirection::South);
        }
        if west {
            data = with_direction(data, BlockDirection::West);
        }
        if east {
            data = with_direction(data, BlockDirection::East);
        }
        data
    }

    pub const fn get_from_level(entry: u16) -> u8 {
        (entry & LEVEL_MASK) as u8
    }

    pub const fn is_from_empty_shape(entry: u16) -> bool {
        entry & FLAG_FROM_EMPTY_SHAPE != 0
    }

    pub const fn is_increase_from_emission(entry: u16) -> bool {
        entry & FLAG_INCREASE_FROM_EMISSION != 0
    }

    pub const fn should_propagate_in_direction(entry: u16, direction: BlockDirection) -> bool {
        entry & direction_bit(direction) != 0
    }

    const fn direction_bit(direction: BlockDirection) -> u16 {
        1 << (direction as u16 + 4)
    }

    const fn with_level(entry: u16, level: u8) -> u16 {
        (entry & !LEVEL_MASK) | (level as u16 & LEVEL_MASK)
    }

    const fn with_direction(entry: u16, direction: BlockDirection) -> u16 {
        entry | direction_bit(direction)
    }

    const fn without_direction(entry: u16, direction: BlockDirection) -> u16 {
        entry & !direction_bit(direction)
    }

    const fn with_empty_shape(entry: u16, from_empty_shape: bool) -> u16 {
        if from_empty_shape {
            entry | FLAG_FROM_EMPTY_SHAPE
        } else {
            entry
        }
    }
}

use queue_entry as qe;

/// Vanilla `LightEngine.PULL_LIGHT_IN_ENTRY`.
const PULL_LIGHT_IN_ENTRY: u16 = qe::decrease_all_directions(1);
const REMOVE_TOP_SKY_SOURCE_ENTRY: u16 = qe::decrease_all_directions(MAX_LEVEL);
const REMOVE_SKY_SOURCE_ENTRY: u16 = qe::decrease_skip_one_direction(MAX_LEVEL, BlockDirection::Up);
const ADD_SKY_SOURCE_ENTRY: u16 =
    qe::increase_skip_one_direction(MAX_LEVEL, false, BlockDirection::Up);

/// One light layer's propagation queues, porting vanilla `BlockLightEngine` and `SkyLightEngine`.
///
/// Sky light in sections without blocks is stored like any other section, so the vanilla
/// `propagateFromEmptySections` shortcut isn't needed.
pub struct LayerLightEngine {
    layer: LightLayer,
    decrease_queue: VecDeque<(BlockPos, u16)>,
    increase_queue: VecDeque<(BlockPos, u16)>,
}

impl LayerLightEngine {
    #[must_use]
    pub const fn new(layer: LightLayer) -> Self {
        Self {
            layer,
            decrease_queue: VecDeque::new(),
            increase_queue: VecDeque::new(),
        }
    }

    fn enqueue_decrease(&mut self, pos: BlockPos, data: u16) {
        self.decrease_queue.push_back((pos, data));
    }

    fn enqueue_increase(&mut self, pos: BlockPos, data: u16) {
        self.increase_queue.push_back((pos, data));
    }

    /// Vanilla `checkNode`: re-evaluates a position whose block changed.
    pub fn check_node<S: LightStorage>(&mut self, storage: &mut S, pos: BlockPos) {
        match self.layer {
            LightLayer::Block => self.check_block_node(storage, pos),
            LightLayer::Sky => self.check_sky_node(storage, pos),
        }
    }

    fn check_block_node<S: LightStorage>(&mut self, storage: &mut S, pos: BlockPos) {
        let Some(old_level) = storage.get_level(LightLayer::Block, pos) else {
            return;
        };
        let state = storage.get_state(pos);
        let emission = state.luminance;
        if emission < old_level {
            storage.set_level(LightLayer::Block, pos, 0);
            self.enqueue_decrease(pos, qe::decrease_all_directions(old_level));
        } else {
            self.enqueue_decrease(pos, PULL_LIGHT_IN_ENTRY);
        }
        if emission > 0 {
            self.enqueue_increase(
                pos,
                qe::increase_light_from_emission(emission, LightEngine::is_empty_shape(state)),
            );
        }
    }

    fn check_sky_node<S: LightStorage>(&mut self, storage: &mut S, pos: BlockPos) {
        let (x, y, z) = (pos.0.x, pos.0.y, pos.0.z);
        let lowest_source_y = storage.lowest_sky_source_y(x, z, i32::MAX);
        if lowest_source_y != i32::MAX {
            self.update_sources_in_column(storage, x, z, lowest_source_y);
        }

        let Some(old_level) = storage.get_level(LightLayer::Sky, pos) else {
            return;
        };
        if y >= lowest_source_y {
            self.enqueue_decrease(pos, REMOVE_SKY_SOURCE_ENTRY);
            self.enqueue_increase(pos, ADD_SKY_SOURCE_ENTRY);
        } else if old_level > 0 {
            storage.set_level(LightLayer::Sky, pos, 0);
            self.enqueue_decrease(pos, qe::decrease_all_directions(old_level));
        } else {
            self.enqueue_decrease(pos, PULL_LIGHT_IN_ENTRY);
        }
    }

    fn update_sources_in_column<S: LightStorage>(
        &mut self,
        storage: &mut S,
        x: i32,
        z: i32,
        lowest_source_y: i32,
    ) {
        let world_bottom_y = storage.bottom_y();
        self.remove_sources_below(storage, x, z, lowest_source_y, world_bottom_y);
        self.add_sources_above(storage, x, z, lowest_source_y, world_bottom_y);
    }

    fn remove_sources_below<S: LightStorage>(
        &mut self,
        storage: &mut S,
        x: i32,
        z: i32,
        lowest_source_y: i32,
        world_bottom_y: i32,
    ) {
        if lowest_source_y <= world_bottom_y {
            return;
        }
        let top_y = storage.top_y();
        for y in (world_bottom_y..(lowest_source_y.min(top_y))).rev() {
            let pos = BlockPos(Vector3::new(x, y, z));
            if storage.get_level(LightLayer::Sky, pos) != Some(MAX_LEVEL) {
                return;
            }
            storage.set_level(LightLayer::Sky, pos, 0);
            self.enqueue_decrease(
                pos,
                if y == lowest_source_y - 1 {
                    REMOVE_TOP_SKY_SOURCE_ENTRY
                } else {
                    REMOVE_SKY_SOURCE_ENTRY
                },
            );
        }
    }

    fn add_sources_above<S: LightStorage>(
        &mut self,
        storage: &mut S,
        x: i32,
        z: i32,
        lowest_source_y: i32,
        world_bottom_y: i32,
    ) {
        let neighbor_lowest_source_y = storage
            .lowest_sky_source_y(x - 1, z, i32::MIN)
            .max(storage.lowest_sky_source_y(x + 1, z, i32::MIN))
            .max(storage.lowest_sky_source_y(x, z - 1, i32::MIN))
            .max(storage.lowest_sky_source_y(x, z + 1, i32::MIN));
        let start_y = lowest_source_y.max(world_bottom_y);
        for y in start_y..storage.top_y() {
            let pos = BlockPos(Vector3::new(x, y, z));
            match storage.get_level(LightLayer::Sky, pos) {
                None => continue,
                Some(MAX_LEVEL) => return,
                Some(_) => {}
            }
            storage.set_level(LightLayer::Sky, pos, MAX_LEVEL);
            if y < neighbor_lowest_source_y || y == lowest_source_y {
                self.enqueue_increase(pos, ADD_SKY_SOURCE_ENTRY);
            }
        }
    }

    /// Vanilla `runLightUpdates`: all decreases, then all increases.
    pub fn run_updates<S: LightStorage>(&mut self, storage: &mut S) {
        while let Some((pos, data)) = self.decrease_queue.pop_front() {
            self.propagate_decrease(storage, pos, data);
        }
        while let Some((pos, data)) = self.increase_queue.pop_front() {
            let Some(mut from_level) = storage.get_level(self.layer, pos) else {
                continue;
            };
            let from_target_level = qe::get_from_level(data);
            if qe::is_increase_from_emission(data) && from_level < from_target_level {
                storage.set_level(self.layer, pos, from_target_level);
                from_level = from_target_level;
            }
            if from_level == from_target_level {
                self.propagate_increase(storage, pos, data, from_level);
            }
        }
    }

    fn propagate_increase<S: LightStorage>(
        &mut self,
        storage: &mut S,
        from_pos: BlockPos,
        increase_data: u16,
        from_level: u8,
    ) {
        let mut from_state: Option<&'static BlockState> = None;
        for direction in PROPAGATION_DIRECTIONS {
            if !qe::should_propagate_in_direction(increase_data, direction) {
                continue;
            }
            let to_pos = from_pos.offset(direction.to_offset());
            let Some(to_level) = storage.get_level(self.layer, to_pos) else {
                continue;
            };
            let max_possible_new_to_level = from_level.saturating_sub(1);
            if max_possible_new_to_level <= to_level {
                continue;
            }
            let to_state = storage.get_state(to_pos);
            let new_to_level = from_level.saturating_sub(LightEngine::get_opacity(to_state));
            if new_to_level <= to_level {
                continue;
            }
            let from_state = *from_state.get_or_insert_with(|| {
                if qe::is_from_empty_shape(increase_data) {
                    BlockStateId::AIR.to_state()
                } else {
                    storage.get_state(from_pos)
                }
            });
            if LightEngine::shape_occludes(from_state, to_state, direction) {
                continue;
            }
            storage.set_level(self.layer, to_pos, new_to_level);
            if new_to_level > 1 {
                self.enqueue_increase(
                    to_pos,
                    qe::increase_skip_one_direction(
                        new_to_level,
                        LightEngine::is_empty_shape(to_state),
                        direction.opposite(),
                    ),
                );
            }
        }
    }

    fn propagate_decrease<S: LightStorage>(
        &mut self,
        storage: &mut S,
        from_pos: BlockPos,
        decrease_data: u16,
    ) {
        let old_from_level = qe::get_from_level(decrease_data);
        for direction in PROPAGATION_DIRECTIONS {
            if !qe::should_propagate_in_direction(decrease_data, direction) {
                continue;
            }
            let to_pos = from_pos.offset(direction.to_offset());
            let Some(to_level) = storage.get_level(self.layer, to_pos) else {
                continue;
            };
            if to_level == 0 {
                continue;
            }
            if i32::from(to_level) > i32::from(old_from_level) - 1 {
                self.enqueue_increase(
                    to_pos,
                    qe::increase_only_one_direction(to_level, false, direction.opposite()),
                );
                continue;
            }
            match self.layer {
                LightLayer::Block => {
                    let to_state = storage.get_state(to_pos);
                    let to_emission = to_state.luminance;
                    storage.set_level(self.layer, to_pos, 0);
                    if to_emission < to_level {
                        self.enqueue_decrease(
                            to_pos,
                            qe::decrease_skip_one_direction(to_level, direction.opposite()),
                        );
                    }
                    if to_emission > 0 {
                        self.enqueue_increase(
                            to_pos,
                            qe::increase_light_from_emission(
                                to_emission,
                                LightEngine::is_empty_shape(to_state),
                            ),
                        );
                    }
                }
                LightLayer::Sky => {
                    storage.set_level(self.layer, to_pos, 0);
                    self.enqueue_decrease(
                        to_pos,
                        qe::decrease_skip_one_direction(to_level, direction.opposite()),
                    );
                }
            }
        }
    }
}

/// Vanilla `ChunkSkyLightSources.findLowestSourceY` for one column, scanning down from `top_y`.
///
/// Returns the y of the lowest sky light source, or `i32::MIN` if the whole column is one
/// (`extendSourcesBelowWorld`).
pub fn find_lowest_sky_source_y(
    bottom_y: i32,
    top_y: i32,
    mut state_at: impl FnMut(i32) -> &'static BlockState,
) -> i32 {
    let mut top_state = BlockStateId::AIR.to_state();
    for y in (bottom_y..top_y).rev() {
        let bottom_state = state_at(y);
        if LightEngine::is_edge_occluded(top_state, bottom_state) {
            return y + 1;
        }
        top_state = bottom_state;
    }
    i32::MIN
}

/// Light access for one chunk's initial lighting during generation.
struct CacheLightStorage<'a> {
    cache: &'a mut Cache,
    bottom_y: i32,
    top_y: i32,
    /// Lowest sky source of each column in the center chunk and its one-block border.
    lowest_sources: [i32; 18 * 18],
    lowest_sources_min_x: i32,
    lowest_sources_min_z: i32,
}

impl CacheLightStorage<'_> {
    const fn chunk_index(&self, pos: BlockPos) -> Option<usize> {
        let rel_x = (pos.0.x >> 4) - self.cache.x;
        let rel_z = (pos.0.z >> 4) - self.cache.z;
        if rel_x < 0 || rel_x >= self.cache.size || rel_z < 0 || rel_z >= self.cache.size {
            return None;
        }
        Some((rel_x * self.cache.size + rel_z) as usize)
    }

    fn compute_lowest_source_y(&self, x: i32, z: i32) -> Option<i32> {
        let index = self.chunk_index(BlockPos(Vector3::new(x, 0, z)))?;
        let (local_x, local_z) = ((x & 15) as usize, (z & 15) as usize);
        Some(match &self.cache.chunks[index] {
            Chunk::Proto(chunk) => {
                // Air never stops sky light, so skip the sections nothing was placed in.
                let top_y = self.top_y.min(self.bottom_y + chunk.populated_height());
                find_lowest_sky_source_y(self.bottom_y, top_y, |y| {
                    chunk
                        .get_block_state_raw(local_x as i32, y - self.bottom_y, local_z as i32)
                        .to_state()
                })
            }
            Chunk::Level(chunk) => find_lowest_sky_source_y(self.bottom_y, self.top_y, |y| {
                chunk
                    .section
                    .get_block_absolute_y(local_x, y, local_z)
                    .unwrap_or(BlockStateId::AIR)
                    .to_state()
            }),
        })
    }
}

impl LightStorage for CacheLightStorage<'_> {
    fn get_level(&self, layer: LightLayer, pos: BlockPos) -> Option<u8> {
        if pos.0.y < self.bottom_y || pos.0.y >= self.top_y {
            return None;
        }
        self.chunk_index(pos)?;
        Some(match layer {
            LightLayer::Sky => get_sky_light(self.cache, pos),
            LightLayer::Block => get_block_light(self.cache, pos),
        })
    }

    fn set_level(&mut self, layer: LightLayer, pos: BlockPos, level: u8) {
        match layer {
            LightLayer::Sky => set_sky_light(self.cache, pos, level),
            LightLayer::Block => set_block_light(self.cache, pos, level),
        }
    }

    fn get_state(&self, pos: BlockPos) -> &'static BlockState {
        GenerationCache::get_block_state(self.cache, &pos.0).to_state()
    }

    fn lowest_sky_source_y(&self, x: i32, z: i32, default: i32) -> i32 {
        let (dx, dz) = (x - self.lowest_sources_min_x, z - self.lowest_sources_min_z);
        if (0..18).contains(&dx) && (0..18).contains(&dz) {
            return self.lowest_sources[(dx * 18 + dz) as usize];
        }
        self.compute_lowest_source_y(x, z).unwrap_or(default)
    }

    fn bottom_y(&self) -> i32 {
        self.bottom_y
    }

    fn top_y(&self) -> i32 {
        self.top_y
    }
}

/// Initial lighting of a generated chunk, like vanilla `LightEngine.propagateLightSources`.
pub struct LightEngine {
    block_light: LayerLightEngine,
    sky_light: LayerLightEngine,
}

impl LightEngine {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            block_light: LayerLightEngine::new(LightLayer::Block),
            sky_light: LayerLightEngine::new(LightLayer::Sky),
        }
    }

    pub fn initialize_light(
        &mut self,
        cache: &mut Cache,
        config: &LightingEngineConfig,
        has_skylight: bool,
    ) {
        if *config != LightingEngineConfig::Default {
            return;
        }

        let should_skip = {
            let center_chunk = cache.get_center_chunk();
            center_chunk.stage >= crate::chunk_system::chunk_state::StagedChunkEnum::Lighting
        };
        if should_skip {
            return;
        }

        // Light that reached this chunk before its blocks were final is recomputed from scratch;
        // light from lit neighbors is pulled back in below.
        let center = cache.get_center_chunk_mut();
        for section in &mut center.light.block_light {
            *section = LightContainer::new_empty(0);
        }
        for section in &mut center.light.sky_light {
            *section = LightContainer::new_empty(0);
        }
        let (chunk_x, chunk_z) = (center.x, center.z);

        let bottom_y = i32::from(cache.bottom_y());
        let top_y = bottom_y + i32::from(cache.height());
        let mut storage = CacheLightStorage {
            cache,
            bottom_y,
            top_y,
            lowest_sources: [i32::MIN; 18 * 18],
            lowest_sources_min_x: chunk_x * 16 - 1,
            lowest_sources_min_z: chunk_z * 16 - 1,
        };

        if has_skylight {
            for dx in 0..18 {
                for dz in 0..18 {
                    let (x, z) = (
                        storage.lowest_sources_min_x + dx,
                        storage.lowest_sources_min_z + dz,
                    );
                    storage.lowest_sources[(dx * 18 + dz) as usize] =
                        storage.compute_lowest_source_y(x, z).unwrap_or(i32::MIN);
                }
            }
            self.sky_light
                .propagate_sky_light_sources(&mut storage, chunk_x, chunk_z);
            self.sky_light.pull_light_in(&storage, chunk_x, chunk_z);
            self.sky_light.run_updates(&mut storage);
        }

        self.block_light
            .propagate_block_light_sources(&storage, chunk_x, chunk_z);
        self.block_light.pull_light_in(&storage, chunk_x, chunk_z);
        self.block_light.run_updates(&mut storage);

        // Most sections end up with one value, full sky light above the terrain or darkness
        // below it, and need no 2 KiB array for that.
        let light = &mut cache.get_center_chunk_mut().light;
        for section in light
            .sky_light
            .iter_mut()
            .chain(light.block_light.iter_mut())
        {
            section.compact();
        }
    }

    /// Checks if a block state has an empty shape for light occlusion, matching vanilla `LightEngine.isEmptyShape`.
    #[inline]
    #[must_use]
    pub const fn is_empty_shape(state: &BlockState) -> bool {
        !state.can_occlude() || !state.use_shape_for_light_occlusion()
    }

    /// Checks if two block states have different light properties, matching vanilla `LightEngine.hasDifferentLightProperties`.
    #[inline]
    #[must_use]
    pub const fn has_different_light_properties(
        old_state: &BlockState,
        new_state: &BlockState,
    ) -> bool {
        if old_state.id.as_u16() == new_state.id.as_u16() {
            return false;
        }

        old_state.opacity != new_state.opacity
            || old_state.luminance != new_state.luminance
            || new_state.use_shape_for_light_occlusion()
            || old_state.use_shape_for_light_occlusion()
    }

    /// Returns the minimum light dampening for this block state, matching vanilla `LightEngine.getOpacity`.
    #[inline]
    #[must_use]
    pub const fn get_opacity(state: &BlockState) -> u8 {
        if state.opacity > MIN_OPACITY {
            state.opacity
        } else {
            MIN_OPACITY
        }
    }

    /// Whether the touching faces of the two states block light between them, matching vanilla
    /// `LightEngine.shapeOccludes`.
    #[inline]
    #[must_use]
    pub const fn shape_occludes(
        from_state: &BlockState,
        to_state: &BlockState,
        direction: BlockDirection,
    ) -> bool {
        from_state
            .light_occlusion_face(direction)
            .occludes_with(to_state.light_occlusion_face(direction.opposite()))
    }

    /// Calculates the light dampening between two blocks, matching vanilla `LightEngine.getLightDampeningInto`.
    ///
    /// Vanilla merges the faces of the full occlusion shapes; on the 1/16 grid that is the same as
    /// comparing the precomputed face shapes.
    #[inline]
    #[must_use]
    pub const fn get_light_dampening_into(
        from_state: &BlockState,
        to_state: &BlockState,
        direction: BlockDirection,
        simple_opacity: u8,
    ) -> u8 {
        if Self::shape_occludes(from_state, to_state, direction) {
            16
        } else {
            simple_opacity
        }
    }

    /// Vanilla `ChunkSkyLightSources.isEdgeOccluded`: whether sky light stops going down from
    /// `top_state` into `bottom_state`.
    #[inline]
    #[must_use]
    pub const fn is_edge_occluded(top_state: &BlockState, bottom_state: &BlockState) -> bool {
        bottom_state.opacity != 0
            || Self::shape_occludes(top_state, bottom_state, BlockDirection::Down)
    }
}

impl LayerLightEngine {
    /// Vanilla `BlockLightEngine.propagateLightSources`.
    fn propagate_block_light_sources<S: LightStorage>(
        &mut self,
        storage: &S,
        chunk_x: i32,
        chunk_z: i32,
    ) {
        for y in storage.bottom_y()..storage.top_y() {
            for z in 0..16 {
                for x in 0..16 {
                    let pos = BlockPos(Vector3::new(chunk_x * 16 + x, y, chunk_z * 16 + z));
                    let state = storage.get_state(pos);
                    if state.luminance > 0 {
                        self.enqueue_increase(
                            pos,
                            qe::increase_light_from_emission(
                                state.luminance,
                                LightEngine::is_empty_shape(state),
                            ),
                        );
                    }
                }
            }
        }
    }

    /// Vanilla `SkyLightEngine.propagateLightSources`.
    fn propagate_sky_light_sources<S: LightStorage>(
        &mut self,
        storage: &mut S,
        chunk_x: i32,
        chunk_z: i32,
    ) {
        let (min_x, min_z) = (chunk_x * 16, chunk_z * 16);
        let (bottom_y, top_y) = (storage.bottom_y(), storage.top_y());
        for z in min_z..min_z + 16 {
            for x in min_x..min_x + 16 {
                let lowest_source_y = storage.lowest_sky_source_y(x, z, i32::MIN);
                let north = storage.lowest_sky_source_y(x, z - 1, i32::MIN);
                let south = storage.lowest_sky_source_y(x, z + 1, i32::MIN);
                let west = storage.lowest_sky_source_y(x - 1, z, i32::MIN);
                let east = storage.lowest_sky_source_y(x + 1, z, i32::MIN);
                let neighbor_lowest_source_y = north.max(south).max(west).max(east);
                for y in (lowest_source_y.max(bottom_y)..top_y).rev() {
                    let pos = BlockPos(Vector3::new(x, y, z));
                    storage.set_level(LightLayer::Sky, pos, MAX_LEVEL);
                    if y == lowest_source_y || y < neighbor_lowest_source_y {
                        self.enqueue_increase(
                            pos,
                            qe::increase_sky_source_in_directions(
                                y == lowest_source_y,
                                y < north,
                                y < south,
                                y < west,
                                y < east,
                            ),
                        );
                    }
                }
            }
        }
    }

    /// Spreads the light already stored right outside the chunk into it, so light from neighbors
    /// that were lit first comes back after the chunk's own light is reset.
    fn pull_light_in<S: LightStorage>(&mut self, storage: &S, chunk_x: i32, chunk_z: i32) {
        let (min_x, min_z) = (chunk_x * 16, chunk_z * 16);
        for y in storage.bottom_y()..storage.top_y() {
            for i in 0..16 {
                for (x, z, toward) in [
                    (min_x + i, min_z - 1, BlockDirection::South),
                    (min_x + i, min_z + 16, BlockDirection::North),
                    (min_x - 1, min_z + i, BlockDirection::East),
                    (min_x + 16, min_z + i, BlockDirection::West),
                ] {
                    let pos = BlockPos(Vector3::new(x, y, z));
                    if let Some(level) = storage.get_level(self.layer, pos)
                        && level > 1
                    {
                        self.enqueue_increase(
                            pos,
                            qe::increase_only_one_direction(level, false, toward),
                        );
                    }
                }
            }
        }
    }
}

impl Default for LightEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::LightEngine;
    use pumpkin_data::block_properties::{SlabType, WhiteWoolSlabLikeProperties};
    use pumpkin_data::{Block, BlockDirection, BlockState};

    fn oak_slab(slab_type: SlabType) -> &'static BlockState {
        let mut props = WhiteWoolSlabLikeProperties::default(&Block::OAK_SLAB);
        props.r#type = slab_type;
        props.to_state_id(&Block::OAK_SLAB).to_state()
    }

    #[test]
    fn shapes_occlude_light_like_vanilla() {
        let grass = Block::GRASS_BLOCK.default_state;
        let air = Block::AIR.default_state;
        let dampening = |above: &BlockState| {
            LightEngine::get_light_dampening_into(grass, above, BlockDirection::Up, above.opacity)
        };
        // SpreadingSnowyBlock.canStayAlive: dirt under a path or a bottom slab can't be grass.
        assert_eq!(dampening(Block::DIRT_PATH.default_state), 16);
        assert_eq!(dampening(oak_slab(SlabType::Bottom)), 16);
        assert_eq!(dampening(oak_slab(SlabType::Top)), 0);
        assert_eq!(dampening(air), 0);

        // Sky light goes down into a bottom slab but not out of its full bottom face.
        let bottom = oak_slab(SlabType::Bottom);
        assert!(!LightEngine::is_edge_occluded(air, bottom));
        assert!(LightEngine::is_edge_occluded(bottom, air));
        // A top slab over a bottom slab leaves no full face between them.
        let top = oak_slab(SlabType::Top);
        assert!(!LightEngine::shape_occludes(
            top,
            bottom,
            BlockDirection::Down
        ));
        assert!(LightEngine::shape_occludes(top, air, BlockDirection::Up));
    }
}
