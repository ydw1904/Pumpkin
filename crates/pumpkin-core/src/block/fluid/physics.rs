use crate::world::World;
use pumpkin_data::fluid::Fluid;
use pumpkin_data::tag::Taggable;
use pumpkin_data::{Block, BlockDirection, tag};
use pumpkin_util::math::position::BlockPos;

/// Lowest height at which lava gives way to water (`LavaFluid.canBeReplacedWith`).
const LAVA_REPLACEABLE_BY_WATER_HEIGHT: f32 = 0.444_444_45;

/// Mirrors vanilla `FlowingFluid.canHoldFluid` for flowing fluid.
///
/// A `LiquidBlockContainer` passes `canHoldAnyFluid`, but its `canPlaceLiquid` rejects flowing
/// fluid, so it never holds it. Vanilla only waterlogs one when source water reaches it through
/// source conversion, which Pumpkin's flow doesn't do yet.
#[must_use]
pub fn can_hold_fluid(block: &Block) -> bool {
    block.has_tag(&tag::Block::MINECRAFT_WASHED_AWAY_BY_FLUIDS) && !is_liquid_block_container(block)
}

/// Vanilla `FluidState.canBeReplacedWith` for the fluid at `pos`.
///
/// No fluid gives way to anything, water only from above and not to water (`WaterFluid`), and
/// lava only to water and only when at least 4/9 high (`LavaFluid`).
#[must_use]
pub fn can_be_replaced_with(
    world: &World,
    pos: &BlockPos,
    other: &Fluid,
    direction: BlockDirection,
) -> bool {
    let (fluid, state) = world.get_fluid_and_fluid_state(pos);
    let other_is_water = other.has_tag(&tag::Fluid::MINECRAFT_WATER);
    if fluid.has_tag(&tag::Fluid::MINECRAFT_WATER) {
        direction == BlockDirection::Down && !other_is_water
    } else if fluid.has_tag(&tag::Fluid::MINECRAFT_LAVA) {
        world.get_fluid_height(pos, fluid, &state) >= LAVA_REPLACEABLE_BY_WATER_HEIGHT
            && other_is_water
    } else {
        true
    }
}

/// Blocks implementing vanilla's `LiquidBlockContainer`: every `SimpleWaterloggedBlock`, plus
/// kelp and seagrass.
fn is_liquid_block_container(block: &Block) -> bool {
    block.is_waterloggable()
        || block == &Block::KELP
        || block == &Block::KELP_PLANT
        || block == &Block::SEAGRASS
        || block == &Block::TALL_SEAGRASS
}
