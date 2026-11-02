use crate::block::{
    BlockBehaviour, GetStateForNeighborUpdateArgs, OnPlaceArgs, PathComputationType,
};
use pumpkin_data::fluid::Fluid;
use pumpkin_data::{BlockDirection, BlockState, BlockStateId};
use pumpkin_macros::pumpkin_block_from_tag;
use pumpkin_world::generation::update_shape::{update_wall_shape, wall_connects_to, wall_shape};
use pumpkin_world::tick::TickPriority;

type WallProperties = pumpkin_data::block_properties::ResinBrickWallLikeProperties;

#[pumpkin_block_from_tag("minecraft:walls")]
pub struct WallBlock;

impl BlockBehaviour for WallBlock {
    fn on_place(&self, args: OnPlaceArgs<'_>) -> BlockStateId {
        let mut wall_props = WallProperties::default(args.block);
        wall_props.waterlogged = args.replacing.water_source();

        // Vanilla `WallBlock.getStateForPlacement`
        let connections = [
            BlockDirection::North,
            BlockDirection::East,
            BlockDirection::South,
            BlockDirection::West,
        ]
        .map(|direction| {
            let neighbor = args
                .world
                .get_block_state_id(&args.position.offset(direction.to_offset()));
            wall_connects_to(neighbor, direction.opposite())
        });
        let above_pos = args.position.up();
        wall_shape(
            wall_props.to_state_id(args.block),
            above_pos.0,
            args.world.get_block_state_id(&above_pos),
            connections,
        )
    }

    fn get_state_for_neighbor_update(
        &self,
        args: GetStateForNeighborUpdateArgs<'_>,
    ) -> BlockStateId {
        let wall_props = WallProperties::from_state_id(args.state_id);
        if wall_props.waterlogged {
            args.world.schedule_fluid_tick(
                &Fluid::WATER,
                *args.position,
                Fluid::WATER.flow_speed,
                TickPriority::Normal,
            );
        }
        let above_pos = args.position.up();
        let above_state = if args.direction == BlockDirection::Up {
            args.neighbor_state_id
        } else {
            args.world.get_block_state_id(&above_pos)
        };
        update_wall_shape(
            args.state_id,
            args.direction,
            args.neighbor_state_id,
            above_pos.0,
            above_state,
        )
    }

    fn is_pathfindable(&self, _state: &BlockState, _computation_type: PathComputationType) -> bool {
        false
    }
}
