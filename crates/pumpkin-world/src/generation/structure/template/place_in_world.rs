//! The block placement half of vanilla `StructureTemplate.placeInWorld`, shared by every
//! template-based structure and feature once its own processors have picked the blocks.

use pumpkin_data::fluid::Fluid;
use pumpkin_data::{BlockId, BlockState, BlockStateId};
use pumpkin_util::math::vector3::Vector3;
use rustc_hash::FxHashSet;

use super::BlockPlacer;
use crate::generation::update_shape::{
    DiscreteVoxelShape, fluid_state, update_from_neighbour_shapes, update_shape_at_edge,
};

/// Vanilla's fill pass looks for a water source in these directions, in this order.
const FILL_DIRECTIONS: [pumpkin_data::BlockDirection; 5] = [
    pumpkin_data::BlockDirection::Up,
    pumpkin_data::BlockDirection::North,
    pumpkin_data::BlockDirection::East,
    pumpkin_data::BlockDirection::South,
    pumpkin_data::BlockDirection::West,
];

/// The state vanilla `StructureTemplate.placeInWorld` keeps while it places a template's blocks:
/// [`Self::set_block`] for each block that passed the processors, then [`Self::finish`].
pub struct TemplateBlockPlacement {
    apply_waterlogging: bool,
    known_shape: bool,
    placed: Vec<Vector3<i32>>,
    to_fill: Vec<Vector3<i32>>,
    locked_fluids: FxHashSet<Vector3<i32>>,
}

impl TemplateBlockPlacement {
    /// `apply_waterlogging` is `StructurePlaceSettings.shouldApplyWaterlogging`. `known_shape`
    /// is `getKnownShape`; vanilla only sets it for jigsaw pieces, which also place with
    /// `UPDATE_KNOWN_SHAPE` (flags 18), while other templates use flags 2 or 260.
    #[must_use]
    pub fn new(apply_waterlogging: bool, known_shape: bool) -> Self {
        Self {
            apply_waterlogging,
            known_shape,
            placed: Vec::new(),
            to_fill: Vec::new(),
            locked_fluids: FxHashSet::default(),
        }
    }

    /// Places one processed template block, waterlogging it when it replaces water.
    pub fn set_block(
        &mut self,
        placer: &mut (impl BlockPlacer + ?Sized),
        pos: Vector3<i32>,
        state: &'static BlockState,
    ) {
        let previous_fluid = self
            .apply_waterlogging
            .then(|| fluid_state(placer.get_block_state(&pos)));
        if self.known_shape {
            placer.set_block_state(&pos, state);
        } else {
            placer.set_block_state_with_post_processing(&pos, state);
        }
        self.placed.push(pos);

        if let Some((previous_fluid, previous_is_source)) = previous_fluid {
            if fluid_state(state.id).1 {
                self.locked_fluids.insert(pos);
            } else if is_liquid_block_container(state.id) {
                place_liquid(placer, pos, state.id, previous_fluid);
                if !previous_is_source {
                    self.to_fill.push(pos);
                }
            }
        }
    }

    /// The rest of `placeInWorld`: waterlogs blocks next to water sources, then updates the
    /// shapes at the template's edge and of every placed block. Those updates start the fluid
    /// ticks that let water flow into air pockets the template left.
    pub fn finish(mut self, level: &mut (impl BlockPlacer + ?Sized)) {
        let mut filled = true;
        while filled && !self.to_fill.is_empty() {
            filled = false;
            self.to_fill.retain(|&pos| {
                let (mut to_place, mut to_place_is_source) =
                    fluid_state(level.get_block_state(&pos));
                for direction in FILL_DIRECTIONS {
                    if to_place_is_source {
                        break;
                    }
                    let neighbor_pos = pos + direction.to_offset();
                    let (neighbor, neighbor_is_source) =
                        fluid_state(level.get_block_state(&neighbor_pos));
                    if neighbor_is_source && !self.locked_fluids.contains(&neighbor_pos) {
                        to_place = neighbor;
                        to_place_is_source = true;
                    }
                }
                if to_place_is_source {
                    let state = level.get_block_state(&pos);
                    if is_liquid_block_container(state) {
                        place_liquid(level, pos, state, to_place);
                        filled = true;
                        return false;
                    }
                }
                true
            });
        }

        if self.known_shape {
            return;
        }
        let Some(mut shape) = DiscreteVoxelShape::covering(&self.placed) else {
            return;
        };
        for &pos in &self.placed {
            shape.fill(pos);
        }
        update_shape_at_edge(level, &shape);

        for &pos in &self.placed {
            let state = level.get_block_state(&pos);
            let new_state = update_from_neighbour_shapes(level, pos, state);
            if new_state != state {
                // Vanilla adds `UPDATE_KNOWN_SHAPE` here. `updateNeighborsAt` does nothing in a
                // `WorldGenRegion`.
                level.set_block_state(&pos, new_state.to_state());
            }
        }
    }
}

/// Whether the block implements vanilla `LiquidBlockContainer`.
fn is_liquid_block_container(state: BlockStateId) -> bool {
    let block = state.to_block();
    block.is_waterloggable()
        || matches!(
            block.id,
            BlockId::KELP | BlockId::KELP_PLANT | BlockId::SEAGRASS | BlockId::TALL_SEAGRASS
        )
}

/// Vanilla `LiquidBlockContainer.placeLiquid`. Of the containers only `SimpleWaterloggedBlock`
/// does anything: it takes a water source and starts its tick.
fn place_liquid(
    level: &mut (impl BlockPlacer + ?Sized),
    pos: Vector3<i32>,
    state: BlockStateId,
    fluid: &'static Fluid,
) {
    // Only a source has the type `Fluids.WATER`; flowing water is `FLOWING_WATER`.
    if fluid.id != Fluid::WATER.id {
        return;
    }
    // `setBlockAndUpdate` uses flags 3.
    if let Some(waterlogged) = state.to_block().set_waterlogged(state, true) {
        level.set_block_state_with_post_processing(&pos, waterlogged.to_state());
        level.schedule_fluid_tick(&pos, &Fluid::WATER);
    }
}
