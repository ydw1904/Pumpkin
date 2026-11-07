use crate::chunk::format::LightContainer;
use crate::chunk::io::Dirtiable;
use crate::chunk::palette::BlockPalette;
use crate::level::Level;
use crate::lighting::engine::{
    LayerLightEngine, LightLayer, LightStorage, find_lowest_sky_source_y,
};
use pumpkin_data::{BlockState, BlockStateId};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector2::Vector2;
use rustc_hash::{FxHashMap, FxHashSet};
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Light updates for block changes in loaded chunks.
pub struct DynamicLightEngine {
    min_y: i32,
    max_y: i32,
    /// Positions in chunks that weren't loaded yet when light tried to reach them, checked again
    /// once the chunk is loaded. Vanilla's light engine already reaches lit chunks that aren't
    /// full, but Pumpkin's runtime light only sees loaded chunks.
    pending_checks: Mutex<FxHashMap<Vector2<i32>, FxHashSet<BlockPos>>>,
    /// Block checks queued since the last light update, like the `checkBlock` tasks vanilla
    /// `ThreadedLevelLightEngine` hands to its light thread.
    queued_checks: Mutex<QueuedChecks>,
    /// Vanilla `ThreadedLevelLightEngine.scheduled`: an update is running on another thread.
    scheduled: AtomicBool,
    /// Held while queued checks run, so updates run one at a time like on vanilla's single
    /// light executor.
    running: Mutex<()>,
    /// Updates that finished but aren't visible to the chunks yet.
    finished: Mutex<Vec<FinishedUpdate>>,
    /// Sections whose light changed since the last [`Self::take_changed_sections`], like the
    /// `sectionsAffectedByLightUpdates` vanilla reports to `ChunkHolder.sectionLightChanged`.
    changed_sections: Mutex<FxHashMap<Vector2<i32>, ChangedLightSections>>,
}

/// The section Y coordinates of a chunk whose sky or block light changed.
#[derive(Default, Debug)]
pub struct ChangedLightSections {
    pub sky: FxHashSet<i32>,
    pub block: FxHashSet<i32>,
}

/// Positions to check in the order they were queued, each once like vanilla's
/// `blockNodesToCheck` set.
#[derive(Default)]
struct QueuedChecks {
    order: Vec<BlockPos>,
    seen: FxHashSet<BlockPos>,
}

/// A light section: layer, chunk and section index in the chunk.
type SectionKey = (LightLayer, Vector2<i32>, usize);

/// The result of one light update, published to the chunks at once like vanilla
/// `LayerLightSectionStorage.swapSectionMap` publishes `updatingSectionData`.
struct FinishedUpdate {
    sections: Vec<(SectionKey, LightContainer)>,
    /// Sections around every position whose light was set, by layer and section position.
    changed: FxHashSet<(LightLayer, i32, i32, i32)>,
}

/// Light access through the level's loaded chunks. Changes go to copies of the sections, like
/// vanilla's `updatingSectionData`, so game logic keeps reading the last published light while an
/// update runs.
struct LevelLightStorage<'a> {
    level: &'a Level,
    min_y: i32,
    max_y: i32,
    /// Positions light couldn't reach because their chunk isn't loaded.
    unloaded: RefCell<Vec<BlockPos>>,
    /// Copies of the sections read or changed since this storage was created. Later updates in
    /// the same run read these, as the chunks only see an update once it's published.
    sections: RefCell<FxHashMap<SectionKey, LightContainer>>,
    /// Sections changed by the current update.
    written: FxHashSet<SectionKey>,
    /// Sections around every position whose light was set, by layer.
    changed: FxHashSet<(LightLayer, i32, i32, i32)>,
}

impl<'a> LevelLightStorage<'a> {
    fn new(level: &'a Level, min_y: i32, max_y: i32) -> Self {
        Self {
            level,
            min_y,
            max_y,
            unloaded: RefCell::default(),
            sections: RefCell::default(),
            written: FxHashSet::default(),
            changed: FxHashSet::default(),
        }
    }

    /// The section key and the position inside the section, or `None` outside the world.
    const fn locate(
        &self,
        layer: LightLayer,
        pos: BlockPos,
    ) -> Option<(SectionKey, usize, usize, usize)> {
        if pos.0.y < self.min_y || pos.0.y >= self.max_y {
            return None;
        }
        let (chunk_pos, relative) = pos.chunk_and_chunk_relative_position();
        let local_y = (relative.y - self.min_y) as usize;
        Some((
            (layer, chunk_pos, local_y / BlockPalette::SIZE),
            relative.x as usize,
            local_y % BlockPalette::SIZE,
            relative.z as usize,
        ))
    }

    /// Copies the section from its chunk if it isn't copied yet. `Some(false)` if the chunk
    /// isn't loaded, `None` if it has no such section.
    fn load_section(&self, key: SectionKey) -> Option<bool> {
        if self.sections.borrow().contains_key(&key) {
            return Some(true);
        }
        let (layer, chunk_pos, index) = key;
        let Some(section) = self.level.read_chunk_sync(&chunk_pos, |chunk| {
            let light = chunk
                .light_engine
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match layer {
                LightLayer::Sky => &light.sky_light,
                LightLayer::Block => &light.block_light,
            }
            .get(index)
            .cloned()
        }) else {
            return Some(false);
        };
        self.sections.borrow_mut().insert(key, section?);
        Some(true)
    }

    /// The sections the current update changed, and resets the change tracking.
    fn finish_update(&mut self) -> FinishedUpdate {
        let sections = self.sections.get_mut();
        FinishedUpdate {
            sections: self
                .written
                .drain()
                .filter_map(|key| Some((key, sections.get(&key)?.clone())))
                .collect(),
            changed: std::mem::take(&mut self.changed),
        }
    }
}

impl LightStorage for LevelLightStorage<'_> {
    fn get_level(&self, layer: LightLayer, pos: BlockPos) -> Option<u8> {
        let (key, x, y, z) = self.locate(layer, pos)?;
        if !self.load_section(key)? {
            self.unloaded.borrow_mut().push(pos);
            return None;
        }
        self.sections
            .borrow()
            .get(&key)
            .map(|section| section.get(x, y, z))
    }

    fn set_level(&mut self, layer: LightLayer, pos: BlockPos, level: u8) {
        // Vanilla `SectionPos.aroundAndAtBlockPos`: a change at a section edge also affects the
        // neighbouring section.
        let (x, y, z) = (pos.0.x, pos.0.y, pos.0.z);
        for section_x in (x - 1) >> 4..=(x + 1) >> 4 {
            for section_y in (y - 1) >> 4..=(y + 1) >> 4 {
                for section_z in (z - 1) >> 4..=(z + 1) >> 4 {
                    self.changed
                        .insert((layer, section_x, section_y, section_z));
                }
            }
        }
        let Some((key, x, y, z)) = self.locate(layer, pos) else {
            return;
        };
        if self.load_section(key) != Some(true) {
            return;
        }
        if let Some(section) = self.sections.get_mut().get_mut(&key) {
            section.set(x, y, z, level);
            self.written.insert(key);
        }
    }

    fn get_state(&self, pos: BlockPos) -> &'static BlockState {
        self.level.get_block_state(&pos).to_state()
    }

    fn lowest_sky_source_y(&self, x: i32, z: i32, default: i32) -> i32 {
        let (local_x, local_z) = ((x & 15) as usize, (z & 15) as usize);
        self.level
            .read_chunk_sync(&Vector2::new(x >> 4, z >> 4), |chunk| {
                let sections = chunk
                    .section
                    .block_sections
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                // Only scan from the highest section with blocks; air never stops sky light.
                let top_y = sections
                    .iter()
                    .rposition(|section| !section.has_only_air())
                    .map_or(self.min_y, |index| {
                        chunk.section.min_y + (index as i32 + 1) * BlockPalette::SIZE as i32
                    });
                find_lowest_sky_source_y(self.min_y, top_y, |y| {
                    let local_y = (y - chunk.section.min_y) as usize;
                    sections
                        .get(local_y / BlockPalette::SIZE)
                        .map_or(BlockStateId::AIR, |section| {
                            section.get(local_x, local_y % BlockPalette::SIZE, local_z)
                        })
                        .to_state()
                })
            })
            .unwrap_or(default)
    }

    fn bottom_y(&self) -> i32 {
        self.min_y
    }

    fn top_y(&self) -> i32 {
        self.max_y
    }
}

impl DynamicLightEngine {
    #[must_use]
    pub fn new(min_y: i32, max_y: i32) -> Self {
        Self {
            min_y,
            max_y,
            pending_checks: Mutex::default(),
            queued_checks: Mutex::default(),
            scheduled: AtomicBool::new(false),
            running: Mutex::new(()),
            finished: Mutex::default(),
            changed_sections: Mutex::default(),
        }
    }

    /// Queues the light that couldn't reach `chunk` before it was loaded, like vanilla would
    /// have checked it when the change happened.
    pub fn check_pending(&self, chunk: Vector2<i32>) {
        let positions = self
            .pending_checks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&chunk);
        if let Some(positions) = positions {
            self.check_blocks(positions);
        }
    }

    /// Vanilla `ThreadedLevelLightEngine.checkBlock`: queues a light check for a changed block.
    /// The light changes once an update scheduled by [`Self::try_schedule_update`] is published.
    pub fn check_block(&self, pos: BlockPos) {
        self.check_blocks([pos]);
    }

    /// Queues light checks for several changed blocks.
    pub fn check_blocks(&self, positions: impl IntoIterator<Item = BlockPos>) {
        let mut queued = self
            .queued_checks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for pos in positions {
            if queued.seen.insert(pos) {
                queued.order.push(pos);
            }
        }
    }

    /// Vanilla `ThreadedLevelLightEngine.tryScheduleUpdate`: starts updating the queued checks
    /// on `pool` unless an update is already running. The tick doesn't wait for it; its light
    /// becomes visible at a later [`Self::publish_updates`].
    pub fn try_schedule_update(&self, level: &Arc<Level>, pool: &rayon::ThreadPool) {
        if self
            .scheduled
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        // The last run has ended, so a new one must start from all of its light.
        self.publish_updates(level);
        if self
            .queued_checks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .order
            .is_empty()
        {
            self.scheduled.store(false, Ordering::Release);
            return;
        }
        let level = level.clone();
        pool.spawn(move || {
            let engine = &level.light_engine;
            let running = engine
                .running
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            engine.run_updates(&level);
            drop(running);
            engine.scheduled.store(false, Ordering::Release);
        });
    }

    /// Makes the light of the finished updates visible to game logic and to light packets.
    pub fn publish_updates(&self, level: &Level) {
        let finished = std::mem::take(
            &mut *self
                .finished
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        if finished.is_empty() {
            return;
        }
        let mut changed_sections = self
            .changed_sections
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for update in finished {
            for ((layer, chunk_pos, index), section) in update.sections {
                level.loaded_chunks.read(&chunk_pos, |chunk| {
                    let mut light = chunk
                        .light_engine
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    let sections = match layer {
                        LightLayer::Sky => &mut light.sky_light,
                        LightLayer::Block => &mut light.block_light,
                    };
                    if let Some(target) = sections.get_mut(index) {
                        *target = section;
                    }
                    drop(light);
                    if !chunk.is_dirty() {
                        chunk.mark_dirty(true);
                    }
                });
            }
            for (layer, x, y, z) in update.changed {
                let sections = changed_sections.entry(Vector2::new(x, z)).or_default();
                match layer {
                    LightLayer::Sky => sections.sky.insert(y),
                    LightLayer::Block => sections.block.insert(y),
                };
            }
        }
    }

    /// Runs and publishes every queued check on this thread, waiting for a running update first.
    /// For shutdown and tests; ticks use [`Self::try_schedule_update`].
    pub fn run_queued_checks(&self, level: &Arc<Level>) {
        let _running = self
            .running
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.publish_updates(level);
        self.run_updates(level);
        self.publish_updates(level);
    }

    /// Vanilla `ThreadedLevelLightEngine.runUpdate`, repeated while checks are queued. Vanilla
    /// takes at most 1000 checks per update; one update for all of them gives the same light and
    /// is several times faster after a large fill, as the light under it propagates only once.
    fn run_updates(&self, level: &Arc<Level>) {
        let mut storage = LevelLightStorage::new(level, self.min_y, self.max_y);
        loop {
            let positions = std::mem::take(
                &mut *self
                    .queued_checks
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            )
            .order;
            if positions.is_empty() {
                return;
            }
            self.update_lighting(level, &mut storage, &positions);
        }
    }

    /// Returns and clears the sections whose light changed, by chunk.
    pub fn take_changed_sections(&self) -> FxHashMap<Vector2<i32>, ChangedLightSections> {
        std::mem::take(
            &mut *self
                .changed_sections
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }

    /// Updates light after several blocks changed, checking all of them before propagating like
    /// vanilla `LightEngine.runLightUpdates` does for the block checks of one update.
    fn update_lighting(
        &self,
        level: &Arc<Level>,
        storage: &mut LevelLightStorage,
        positions: &[BlockPos],
    ) {
        if let Some(value) = level.lighting_config.fixed_level() {
            for pos in positions {
                self.set_block_light_level(level, pos, value).ok();
                self.set_sky_light_level(level, pos, value).ok();
            }
            return;
        }

        let mut block_light = LayerLightEngine::new(LightLayer::Block);
        for &pos in positions {
            block_light.check_node(storage, pos);
        }
        block_light.run_updates(storage);

        if level.world_gen.load().dimension().has_skylight {
            let mut sky_light = LayerLightEngine::new(LightLayer::Sky);
            for &pos in positions {
                sky_light.check_node(storage, pos);
            }
            sky_light.run_updates(storage);
        }

        let update = storage.finish_update();
        if !update.sections.is_empty() || !update.changed.is_empty() {
            self.finished
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(update);
        }

        let unloaded = std::mem::take(storage.unloaded.get_mut());
        if unloaded.is_empty() {
            return;
        }
        let mut chunks = FxHashSet::default();
        {
            let mut pending = self
                .pending_checks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for pos in unloaded {
                let chunk = pos.chunk_and_chunk_relative_position().0;
                pending.entry(chunk).or_default().insert(pos);
                chunks.insert(chunk);
            }
        }
        // A chunk that got loaded meanwhile won't be announced again.
        for chunk in chunks {
            if level.is_chunk_loaded(&chunk) {
                self.check_pending(chunk);
            }
        }
    }

    pub fn get_block_light_level_sync(&self, level: &Level, position: &BlockPos) -> Option<u8> {
        let (chunk_pos, relative) = position.chunk_and_chunk_relative_position();

        level.read_chunk_sync(&chunk_pos, |chunk| {
            let section_idx = (relative.y - chunk.section.min_y) as usize / 16;
            let light_engine = chunk.light_engine.lock().ok()?;

            light_engine
                .block_light
                .get(section_idx)?
                .get(
                    relative.x as usize,
                    (relative.y - chunk.section.min_y) as usize % 16,
                    relative.z as usize,
                )
                .into()
        })?
    }

    pub fn get_sky_light_level_sync(&self, level: &Level, position: &BlockPos) -> u8 {
        let (chunk_coordinate, relative) = position.chunk_and_chunk_relative_position();
        level
            .read_chunk_sync(&chunk_coordinate, |chunk| {
                let section_index =
                    (relative.y - chunk.section.min_y) as usize / BlockPalette::SIZE;

                let light_engine = chunk
                    .light_engine
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                // Bounds check for section index (lock the light engine)
                if section_index >= light_engine.sky_light.len() {
                    return 15;
                }

                light_engine.sky_light[section_index].get(
                    relative.x as usize,
                    (relative.y - chunk.section.min_y) as usize % BlockPalette::SIZE,
                    relative.z as usize,
                )
            })
            .unwrap_or(0)
    }

    pub fn get_block_light_level(&self, level: &Arc<Level>, position: &BlockPos) -> Option<u8> {
        let (chunk_pos, relative) = position.chunk_and_chunk_relative_position();

        level
            .read_chunk_sync(&chunk_pos, |chunk| {
                let section_idx = (relative.y - chunk.section.min_y) as usize / 16;
                chunk
                    .light_engine
                    .lock()
                    .ok()?
                    .block_light
                    .get(section_idx)
                    .map(|section| {
                        section.get(
                            relative.x as usize,
                            (relative.y - chunk.section.min_y) as usize % 16,
                            relative.z as usize,
                        )
                    })
            })
            .flatten()
    }

    pub fn set_block_light_level(
        &self,
        level: &Arc<Level>,
        position: &BlockPos,
        light_level: u8,
    ) -> Result<(), String> {
        let (chunk_coordinate, relative) = position.chunk_and_chunk_relative_position();
        level.read_chunk_sync(&chunk_coordinate, |chunk| {
            let section_index = (relative.y - chunk.section.min_y) as usize / BlockPalette::SIZE;
            {
                let mut light_engine = chunk
                    .light_engine
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if section_index >= light_engine.block_light.len() {
                    return Err("Invalid section index".to_string());
                }
                let relative_y = (relative.y - chunk.section.min_y) as usize % BlockPalette::SIZE;
                light_engine.block_light[section_index].set(
                    relative.x as usize,
                    relative_y,
                    relative.z as usize,
                    light_level,
                );
            };
            // Mark chunk as dirty so lighting changes are saved to disk
            if !chunk.is_dirty() {
                chunk.mark_dirty(true);
            }
            Ok(())
        });
        Ok(())
    }

    pub fn get_sky_light_level(&self, level: &Arc<Level>, position: &BlockPos) -> u8 {
        let (chunk_coordinate, relative) = position.chunk_and_chunk_relative_position();
        level
            .read_chunk_sync(&chunk_coordinate, |chunk| {
                let section_index =
                    (relative.y - chunk.section.min_y) as usize / BlockPalette::SIZE;

                let light_engine = chunk
                    .light_engine
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                // Bounds check for section index (lock the light engine)
                if section_index >= light_engine.sky_light.len() {
                    return 15;
                }

                light_engine.sky_light[section_index].get(
                    relative.x as usize,
                    (relative.y - chunk.section.min_y) as usize % BlockPalette::SIZE,
                    relative.z as usize,
                )
            })
            .unwrap_or(0)
    }

    pub fn set_sky_light_level(
        &self,
        level: &Arc<Level>,
        position: &BlockPos,
        light_level: u8,
    ) -> Result<(), String> {
        let (chunk_coordinate, relative) = position.chunk_and_chunk_relative_position();
        level.read_chunk_sync(&chunk_coordinate, |chunk| {
            let section_index = (relative.y - chunk.section.min_y) as usize / BlockPalette::SIZE;
            {
                let mut light_engine = chunk
                    .light_engine
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if section_index >= light_engine.sky_light.len() {
                    return Err("Invalid section index".to_string());
                }
                let relative_y = (relative.y - chunk.section.min_y) as usize % BlockPalette::SIZE;
                light_engine.sky_light[section_index].set(
                    relative.x as usize,
                    relative_y,
                    relative.z as usize,
                    light_level,
                );
            };
            // Mark chunk as dirty so lighting changes are saved to disk
            if !chunk.is_dirty() {
                chunk.mark_dirty(true);
            }
            Ok(())
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::{ChunkData, ChunkLight};
    use crate::level::SyncChunk;
    use pumpkin_config::world::LevelConfig;
    use pumpkin_data::Block;
    use pumpkin_data::dimension::Dimension;
    use tempfile::TempDir;

    fn lit_chunk(x: i32, z: i32) -> SyncChunk {
        let mut chunk = ChunkData::empty(x, z);
        let sections = chunk.section.count;
        chunk.light_engine = Mutex::new(ChunkLight {
            sky_light: (0..sections)
                .map(|_| LightContainer::new_empty(15))
                .collect(),
            block_light: (0..sections)
                .map(|_| LightContainer::new_empty(0))
                .collect(),
        });
        Arc::new(chunk)
    }

    #[tokio::test]
    async fn light_reaches_chunks_loaded_after_the_change() {
        let temp_dir = TempDir::new().unwrap();
        let level = Level::from_root_folder(
            &LevelConfig::default(),
            temp_dir.path().to_path_buf(),
            0,
            Dimension::OVERWORLD,
        );
        let origin = Vector2::new(0, 0);
        let neighbor = Vector2::new(1, 0);
        let chunk = lit_chunk(origin.x, origin.y);
        chunk.set_block_absolute_y(15, 64, 8, Block::GLOWSTONE.default_state.id);
        level.loaded_chunks.insert(origin, chunk);
        level.light_engine.check_block(BlockPos::new(15, 64, 8));
        level.light_engine.run_queued_checks(&level);

        let across = BlockPos::new(16, 64, 8);
        level
            .loaded_chunks
            .insert(neighbor, lit_chunk(neighbor.x, neighbor.y));
        assert_eq!(
            level.light_engine.get_block_light_level(&level, &across),
            Some(0)
        );
        level.light_engine.check_pending(neighbor);
        level.light_engine.run_queued_checks(&level);
        assert_eq!(
            level.light_engine.get_block_light_level(&level, &across),
            Some(14)
        );
        level.shutdown().await;
    }

    #[tokio::test]
    async fn light_stays_unchanged_until_the_update_is_published() {
        let temp_dir = TempDir::new().unwrap();
        let level = Level::from_root_folder(
            &LevelConfig::default(),
            temp_dir.path().to_path_buf(),
            0,
            Dimension::OVERWORLD,
        );
        let chunk = lit_chunk(0, 0);
        chunk.set_block_absolute_y(8, 64, 8, Block::GLOWSTONE.default_state.id);
        level.loaded_chunks.insert(Vector2::new(0, 0), chunk);
        level.light_engine.check_block(BlockPos::new(8, 64, 8));
        level.light_engine.run_updates(&level);

        let beside = BlockPos::new(9, 64, 8);
        assert_eq!(
            level.light_engine.get_block_light_level(&level, &beside),
            Some(0)
        );
        level.light_engine.publish_updates(&level);
        assert_eq!(
            level.light_engine.get_block_light_level(&level, &beside),
            Some(14)
        );
        assert!(
            level
                .light_engine
                .take_changed_sections()
                .contains_key(&Vector2::new(0, 0))
        );
        level.shutdown().await;
    }
}
