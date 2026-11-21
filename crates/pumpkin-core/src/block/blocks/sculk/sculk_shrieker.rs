use std::sync::Arc;

use crate::block::entities::sculk_shrieker::SculkShriekerBlockEntity;
use crate::block::{
    BlockBehaviour, BlockMetadata, GetStateForNeighborUpdateArgs, OnEntityStepArgs, OnPlaceArgs,
    OnScheduledTickArgs,
};
use crate::world::World;
use pumpkin_data::{
    BlockId, BlockStateId, block_properties::SculkShriekerLikeProperties, fluid::Fluid,
};
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::tick::TickPriority;
use pumpkin_world::world::BlockFlags;

pub struct SculkShriekerBlock;

impl BlockMetadata for SculkShriekerBlock {
    fn ids() -> Box<[BlockId]> {
        [BlockId::SCULK_SHRIEKER].into()
    }
}

impl SculkShriekerBlock {
    fn with_shrieker(world: &World, pos: &BlockPos, f: impl FnOnce(&SculkShriekerBlockEntity)) {
        if let Some(entity) = world.get_block_entity(pos)
            && let Some(shrieker) = entity.as_any().downcast_ref::<SculkShriekerBlockEntity>()
        {
            f(shrieker);
        }
    }
}

impl BlockBehaviour for SculkShriekerBlock {
    fn on_place(&self, args: OnPlaceArgs<'_>) -> BlockStateId {
        let mut props = SculkShriekerLikeProperties::default(args.block);
        props.shrieking = false;
        props.waterlogged = args.replacing.water_source();
        props.to_state_id(args.block)
    }

    fn get_state_for_neighbor_update(
        &self,
        args: GetStateForNeighborUpdateArgs<'_>,
    ) -> BlockStateId {
        let props = SculkShriekerLikeProperties::from_state_id(args.state_id);
        if props.waterlogged {
            args.world.schedule_fluid_tick(
                &Fluid::WATER,
                *args.position,
                Fluid::WATER.flow_speed,
                TickPriority::Normal,
            );
        }
        args.state_id
    }

    fn on_entity_step(&self, args: OnEntityStepArgs<'_>) {
        if let Some(player) =
            SculkShriekerBlockEntity::try_get_player(args.world, Some(args.entity))
        {
            let world: &Arc<World> = args.world;
            Self::with_shrieker(world, args.position, |shrieker| {
                shrieker.try_shriek(world, Some(player));
            });
        }
    }

    fn on_scheduled_tick(&self, args: OnScheduledTickArgs<'_>) {
        let state = args.world.get_block_state_id(args.position);
        let mut props = SculkShriekerLikeProperties::from_state_id(state);
        if props.shrieking {
            props.shrieking = false;
            args.world.set_block_state(
                args.position,
                props.to_state_id(args.block),
                BlockFlags::NOTIFY_ALL,
            );
            Self::with_shrieker(args.world, args.position, |shrieker| {
                shrieker.try_respond(args.world, state);
            });
        }
    }
}
