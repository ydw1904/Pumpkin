use std::sync::Arc;

use crate::block::entities::beehive::BeehiveBlockEntity;
use crate::block::{BlockBehaviour, BlockMetadata, GetComparatorOutputArgs, OnPlaceArgs};
use crate::entity::EntityBase;
use crate::world::World;
use pumpkin_data::block_properties::BeeNestLikeProperties;
use pumpkin_data::{BlockId, BlockStateId};
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::world::BlockFlags;

pub struct BeehiveBlock;

impl BeehiveBlock {
    /// Vanilla `releaseBeesAndResetHoneyLevel` with `BeeReleaseStatus.BEE_RELEASED` and no
    /// player, as a dispenser's shears do.
    pub fn release_bees_and_reset_honey_level(world: &Arc<World>, pos: &BlockPos) {
        let (block, state_id) = world.get_block_and_state_id(pos);
        let mut props = BeeNestLikeProperties::from_state_id(state_id);
        let facing = props.facing;
        props.honey_level = 0;
        world.set_block_state(pos, props.to_state_id(block), BlockFlags::NOTIFY_ALL);
        if let Some(block_entity) = world.get_block_entity(pos)
            && let Some(beehive) = block_entity.as_any().downcast_ref::<BeehiveBlockEntity>()
        {
            beehive.release_all_occupants(world, facing);
        }
    }
}

impl BlockMetadata for BeehiveBlock {
    fn ids() -> Box<[BlockId]> {
        [BlockId::BEEHIVE, BlockId::BEE_NEST].into()
    }
}

impl BlockBehaviour for BeehiveBlock {
    fn on_place(&self, args: OnPlaceArgs<'_>) -> BlockStateId {
        let mut props = BeeNestLikeProperties::default(args.block);
        props.facing = args.player.get_entity().get_horizontal_facing().opposite();
        props.to_state_id(args.block)
    }

    fn get_comparator_output(&self, args: GetComparatorOutputArgs<'_>) -> Option<u8> {
        let state_id = args.world.get_block_state_id(args.position);
        let props = BeeNestLikeProperties::from_state_id(state_id);
        Some(props.honey_level)
    }
}
