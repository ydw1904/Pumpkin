use std::sync::Arc;

use pumpkin_data::block_properties::BubbleColumnLikeProperties;
use pumpkin_data::fluid::Fluid;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::{Block, BlockDirection, BlockId, BlockStateId};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::tick::TickPriority;
use pumpkin_world::world::BlockFlags;

use crate::block::fluid::{flowing_trait::FlowingFluid, lava::FlowingLava};
use crate::block::{
    BlockBehaviour, BlockMetadata, GetStateForNeighborUpdateArgs, OnEntityCollisionArgs,
    OnNeighborUpdateArgs, OnScheduledTickArgs, PlacedArgs,
};
use crate::entity::Entity;
use crate::world::World;

/// Vanilla `BubbleColumnBlock.CHECK_PERIOD`.
const CHECK_PERIOD: u32 = 5;
/// Vanilla `LiquidBlock.BUBBLE_COLUMN_CHECK_DELAY`.
const BUBBLE_COLUMN_CHECK_DELAY: u32 = 20;

const UPWARD_ACCELERATION: f64 = 0.06;
const UPWARD_MAX_SPEED: f64 = 0.7;
pub(crate) const SURFACE_UPWARD_ACCELERATION: f64 = 0.1;
const SURFACE_UPWARD_MAX_SPEED: f64 = 1.8;
const DOWNWARD_ACCELERATION: f64 = -0.03;
const DOWNWARD_MIN_SPEED: f64 = -0.3;

pub struct BubbleColumnBlock;

impl BlockMetadata for BubbleColumnBlock {
    fn ids() -> Box<[BlockId]> {
        [BlockId::BUBBLE_COLUMN, BlockId::WATER, BlockId::LAVA].into()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BubbleColumnKind {
    Upward,
    Downward,
}

fn bubble_column_state(kind: BubbleColumnKind) -> BlockStateId {
    BubbleColumnLikeProperties {
        r#drag: matches!(kind, BubbleColumnKind::Downward),
    }
    .to_state_id(&Block::BUBBLE_COLUMN)
}

fn kind_from_support(block: &Block) -> Option<BubbleColumnKind> {
    if block.has_tag(&tag::Block::MINECRAFT_ENABLES_BUBBLE_COLUMN_PUSH_UP) {
        Some(BubbleColumnKind::Upward)
    } else if block.has_tag(&tag::Block::MINECRAFT_ENABLES_BUBBLE_COLUMN_DRAG_DOWN) {
        Some(BubbleColumnKind::Downward)
    } else {
        None
    }
}

fn kind_from_state(state: BlockStateId) -> BubbleColumnKind {
    let props = BubbleColumnLikeProperties::from_state_id(state);
    if props.r#drag {
        BubbleColumnKind::Downward
    } else {
        BubbleColumnKind::Upward
    }
}

/// Vanilla `LiquidBlock.shouldBubbleColumnOccupy`.
#[must_use]
pub fn is_source_water_state(state: BlockStateId) -> bool {
    let Some(fluid) = Fluid::from_state_id(state) else {
        return false;
    };

    fluid.has_tag(&tag::Fluid::MINECRAFT_BUBBLE_COLUMN_CAN_OCCUPY)
        && fluid.is_source(state)
        && fluid.states.iter().any(|fluid_state| {
            fluid_state.block_state_id == state && fluid_state.is_still && fluid_state.is_source
        })
}

/// Vanilla `BubbleColumnBlock.canOccupy`.
fn can_occupy(state: BlockStateId) -> bool {
    Block::from_state_id(state) == &Block::BUBBLE_COLUMN || is_source_water_state(state)
}

/// Vanilla `BubbleColumnBlock.getColumnState`.
fn column_state(below_state: BlockStateId, occupy_state: BlockStateId) -> BlockStateId {
    let below_block = Block::from_state_id(below_state);
    if below_block == &Block::BUBBLE_COLUMN {
        below_state
    } else if let Some(kind) = kind_from_support(below_block) {
        bubble_column_state(kind)
    } else if Block::from_state_id(occupy_state) == &Block::BUBBLE_COLUMN {
        Block::WATER.default_state.id
    } else {
        occupy_state
    }
}

/// Vanilla `BubbleColumnBlock.updateColumn`: fills `occupy_at` and the source water above it
/// with the column that `below_state` supports, all at once.
pub fn update_column(world: &Arc<World>, occupy_at: BlockPos, below_state: BlockStateId) {
    let occupy_state = world.get_block_state_id(&occupy_at);
    if !can_occupy(occupy_state) {
        return;
    }
    let column_state = column_state(below_state, occupy_state);
    world.set_block_state(&occupy_at, column_state, BlockFlags::NOTIFY_LISTENERS);
    let mut pos = occupy_at.up();
    while can_occupy(world.get_block_state_id(&pos)) {
        // Vanilla stops when `setBlock` changes nothing.
        if world.set_block_state(&pos, column_state, BlockFlags::NOTIFY_LISTENERS) == column_state {
            return;
        }
        pos = pos.up();
    }
}

/// Vanilla `BubbleColumnBlock.canSurvive`.
fn can_survive(world: &World, position: &BlockPos) -> bool {
    let below = world.get_block(&position.down());
    below == &Block::BUBBLE_COLUMN || kind_from_support(below).is_some()
}

/// Vanilla `LiquidBlock.tryScheduleBubbleBlockColumn`.
fn try_schedule_bubble_column(world: &World, position: &BlockPos, below_state: BlockStateId) {
    if kind_from_support(Block::from_state_id(below_state)).is_some() {
        world.schedule_block_tick(
            &Block::WATER,
            *position,
            BUBBLE_COLUMN_CHECK_DELAY,
            TickPriority::Normal,
        );
    }
}

fn bubble_column_vertical_velocity(
    current_y: f64,
    kind: BubbleColumnKind,
    at_surface: bool,
) -> f64 {
    match (kind, at_surface) {
        (BubbleColumnKind::Upward, true) => {
            (current_y + SURFACE_UPWARD_ACCELERATION).min(SURFACE_UPWARD_MAX_SPEED)
        }
        (BubbleColumnKind::Upward, false) => {
            (current_y + UPWARD_ACCELERATION).min(UPWARD_MAX_SPEED)
        }
        (BubbleColumnKind::Downward, _) => {
            (current_y + DOWNWARD_ACCELERATION).max(DOWNWARD_MIN_SPEED)
        }
    }
}

fn apply_bubble_column_velocity(entity: &Entity, kind: BubbleColumnKind, at_surface: bool) {
    let current = entity.velocity.load();
    entity.velocity.store(Vector3::new(
        current.x,
        bubble_column_vertical_velocity(current.y, kind, at_surface),
        current.z,
    ));
}

const fn kind_of(drag_down: bool) -> BubbleColumnKind {
    if drag_down {
        BubbleColumnKind::Downward
    } else {
        BubbleColumnKind::Upward
    }
}

/// Vanilla `Entity.handleOnAboveBubbleColumn`.
pub(crate) fn handle_on_above_bubble_column(entity: &Entity, drag_down: bool) {
    apply_bubble_column_velocity(entity, kind_of(drag_down), true);
}

/// Vanilla `Entity.handleOnInsideBubbleColumn`.
pub(crate) fn handle_on_inside_bubble_column(entity: &Entity, drag_down: bool) {
    apply_bubble_column_velocity(entity, kind_of(drag_down), false);
}

impl BlockBehaviour for BubbleColumnBlock {
    fn on_entity_collision(&self, args: OnEntityCollisionArgs<'_>) {
        {
            if args.block != &Block::BUBBLE_COLUMN {
                return;
            }

            let drag_down = kind_from_state(args.state.id) == BubbleColumnKind::Downward;
            if args.world.get_block(&args.position.up()) == &Block::AIR {
                args.entity.on_above_bubble_column(drag_down, args.position);
            } else {
                args.entity.on_inside_bubble_column(drag_down);
            }
        }
    }

    fn placed(&self, args: PlacedArgs<'_>) {
        // `LiquidBlock.onPlace`
        if args.block == &Block::WATER && is_source_water_state(args.state_id) {
            let below_state = args.world.get_block_state_id(&args.position.down());
            try_schedule_bubble_column(args.world, args.position, below_state);
        }
    }

    fn on_neighbor_update(&self, args: OnNeighborUpdateArgs<'_>) {
        // `LiquidBlock.neighborChanged`
        if args.block == &Block::WATER {
            let state = args.world.get_block_state_id(args.position);
            if is_source_water_state(state) {
                let below_state = args.world.get_block_state_id(&args.position.down());
                try_schedule_bubble_column(args.world, args.position, below_state);
            }
        }
    }

    fn get_state_for_neighbor_update(
        &self,
        args: GetStateForNeighborUpdateArgs<'_>,
    ) -> BlockStateId {
        if args.block == &Block::BUBBLE_COLUMN {
            // `BubbleColumnBlock.updateShape`
            args.world.schedule_fluid_tick(
                &Fluid::WATER,
                *args.position,
                Fluid::WATER.flow_speed,
                TickPriority::Normal,
            );
            if !can_survive(args.world, args.position)
                || args.direction == BlockDirection::Down
                || (args.direction == BlockDirection::Up
                    && Block::from_state_id(args.neighbor_state_id) != &Block::BUBBLE_COLUMN
                    && can_occupy(args.neighbor_state_id))
            {
                args.world.schedule_block_tick(
                    &Block::BUBBLE_COLUMN,
                    *args.position,
                    CHECK_PERIOD,
                    TickPriority::Normal,
                );
            }
        } else {
            // `LiquidBlock.updateShape`: a source, or a liquid next to one, may flow again.
            let fluid = World::get_fluid_type_from_state_id(args.state_id);
            let is_source = |state| {
                let fluid = World::get_fluid_type_from_state_id(state);
                fluid == &Fluid::WATER || fluid == &Fluid::LAVA
            };
            if is_source(args.state_id) || is_source(args.neighbor_state_id) {
                let delay = if args.block == &Block::LAVA {
                    FlowingLava.get_flow_speed(args.world)
                } else {
                    Fluid::WATER.flow_speed
                };
                args.world
                    .schedule_fluid_tick(fluid, *args.position, delay, TickPriority::Normal);
            }
            if args.direction == BlockDirection::Down && is_source_water_state(args.state_id) {
                try_schedule_bubble_column(args.world, args.position, args.neighbor_state_id);
            }
        }
        args.state_id
    }

    fn on_scheduled_tick(&self, args: OnScheduledTickArgs<'_>) {
        // `BubbleColumnBlock.tick` and `LiquidBlock.tick`
        let state = args.world.get_block_state_id(args.position);
        if args.block == &Block::BUBBLE_COLUMN || is_source_water_state(state) {
            let below_state = args.world.get_block_state_id(&args.position.down());
            update_column(args.world, *args.position, below_state);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn support_tags_map_to_expected_kinds() {
        assert_eq!(
            kind_from_support(&Block::SOUL_SAND),
            Some(BubbleColumnKind::Upward)
        );
        assert_eq!(
            kind_from_support(&Block::MAGMA_BLOCK),
            Some(BubbleColumnKind::Downward)
        );
        assert_eq!(kind_from_support(&Block::STONE), None);
    }
}
