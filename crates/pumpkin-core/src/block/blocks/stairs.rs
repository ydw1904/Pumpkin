use crate::entity::EntityBase;
use pumpkin_data::block_properties::Half;
use pumpkin_data::block_properties::HorizontalFacing;
use pumpkin_data::block_properties::StairsShape;
use pumpkin_data::fluid::Fluid;
use pumpkin_data::{Block, BlockDirection, BlockState, BlockStateId, Mirror, Rotation};
use pumpkin_macros::pumpkin_block_from_tag;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::generation::update_shape::stairs_shape;
use pumpkin_world::tick::TickPriority;

use crate::block::{
    BlockBehaviour, GetStateForNeighborUpdateArgs, OnPlaceArgs, PathComputationType,
};
use crate::world::World;

type StairsProperties = pumpkin_data::block_properties::WhiteWoolStairsLikeProperties;

#[pumpkin_block_from_tag("minecraft:stairs")]
pub struct StairBlock;

impl BlockBehaviour for StairBlock {
    fn on_place(&self, args: OnPlaceArgs<'_>) -> BlockStateId {
        let mut stair_props = StairsProperties::default(args.block);
        stair_props.waterlogged = args.replacing.water_source();

        stair_props.facing = args.player.get_entity().get_horizontal_facing();
        stair_props.half = match args.direction {
            BlockDirection::Up => Half::Top,
            BlockDirection::Down => Half::Bottom,
            _ => match args.use_item_on.cursor_pos.y {
                0.0..=0.5 => Half::Bottom,
                _ => Half::Top,
            },
        };

        stair_props.shape = compute_stair_shape(args.world, args.position, stair_props);

        stair_props.to_state_id(args.block)
    }

    fn get_state_for_neighbor_update(
        &self,
        args: GetStateForNeighborUpdateArgs<'_>,
    ) -> BlockStateId {
        let mut stair_props = StairsProperties::from_state_id(args.state_id);
        if stair_props.waterlogged {
            args.world.schedule_fluid_tick(
                &Fluid::WATER,
                *args.position,
                Fluid::WATER.flow_speed,
                TickPriority::Normal,
            );
        }

        if args.direction.is_horizontal() {
            stair_props.shape = compute_stair_shape(args.world, args.position, stair_props);
            return stair_props.to_state_id(args.block);
        }

        args.state_id
    }

    fn rotate(
        &self,
        block: &Block,
        state_id: BlockStateId,
        rotation: Rotation,
    ) -> &'static BlockState {
        let mut stair_props = StairsProperties::from_state_id(state_id);
        stair_props.facing = rotation.rotate_horizontal(stair_props.facing);
        BlockState::from_id(stair_props.to_state_id(block))
    }

    fn mirror(&self, block: &Block, state_id: BlockStateId, mirror: Mirror) -> &'static BlockState {
        let mut stair_props = StairsProperties::from_state_id(state_id);
        let direction = stair_props.facing;
        let shape = stair_props.shape;

        match mirror {
            Mirror::LeftRight => {
                if matches!(direction, HorizontalFacing::North | HorizontalFacing::South) {
                    stair_props.facing = stair_props.facing.opposite();
                    stair_props.shape = match shape {
                        StairsShape::Straight => StairsShape::Straight,
                        StairsShape::OuterLeft => StairsShape::OuterRight,
                        StairsShape::InnerRight => StairsShape::InnerLeft,
                        StairsShape::InnerLeft => StairsShape::InnerRight,
                        StairsShape::OuterRight => StairsShape::OuterLeft,
                    };
                    return BlockState::from_id(stair_props.to_state_id(block));
                }
            }
            Mirror::FrontBack => {
                if matches!(direction, HorizontalFacing::East | HorizontalFacing::West) {
                    stair_props.facing = stair_props.facing.opposite();
                    stair_props.shape = match shape {
                        StairsShape::Straight => StairsShape::Straight,
                        StairsShape::OuterLeft => StairsShape::OuterRight,
                        StairsShape::InnerRight => StairsShape::InnerRight,
                        StairsShape::InnerLeft => StairsShape::InnerLeft,
                        StairsShape::OuterRight => StairsShape::OuterLeft,
                    };
                    return BlockState::from_id(stair_props.to_state_id(block));
                }
            }
            Mirror::None => {}
        }

        BlockState::from_id(state_id)
    }

    fn is_pathfindable(&self, _state: &BlockState, _computation_type: PathComputationType) -> bool {
        false
    }
}

fn compute_stair_shape(
    world: &World,
    block_pos: &BlockPos,
    props: StairsProperties,
) -> StairsShape {
    stairs_shape(props, |direction| {
        world.get_block_state_id(&block_pos.offset(direction.to_offset()))
    })
}
