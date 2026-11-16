use std::sync::Arc;

use pumpkin_data::BlockStateId;
use pumpkin_data::block_properties::{VaultLikeProperties, VaultState};
use pumpkin_macros::pumpkin_block;

use crate::block::entities::vault::VaultBlockEntity;
use crate::block::registry::BlockActionResult;
use crate::block::{BlockBehaviour, OnPlaceArgs, PlacedArgs, UseWithItemArgs};

#[pumpkin_block("minecraft:vault")]
pub struct VaultBlock;

impl BlockBehaviour for VaultBlock {
    fn on_place(&self, args: OnPlaceArgs<'_>) -> BlockStateId {
        let mut props = VaultLikeProperties::default(args.block);
        props.facing = args
            .player
            .living_entity
            .entity
            .get_horizontal_facing()
            .opposite();
        props.to_state_id(args.block)
    }

    fn placed(&self, args: PlacedArgs<'_>) {
        let entity = VaultBlockEntity::new(*args.position);
        args.world.add_block_entity(Arc::new(entity));
    }

    /// Vanilla `VaultBlock.useItemOn`: an active vault tries the held item as its key.
    fn use_with_item(&self, args: UseWithItemArgs<'_>) -> BlockActionResult {
        let state_id = args.world.get_block_state_id(args.position);
        if args.item_stack.is_empty()
            || VaultLikeProperties::from_state_id(state_id).vault_state != VaultState::Active
        {
            return BlockActionResult::PassToDefaultBlockAction;
        }
        let Some(block_entity) = args.world.get_block_entity(args.position) else {
            return BlockActionResult::PassToDefaultBlockAction;
        };
        let Some(vault) = block_entity.as_any().downcast_ref::<VaultBlockEntity>() else {
            return BlockActionResult::PassToDefaultBlockAction;
        };
        vault.try_insert_key(args.world, args.player, args.item_stack);
        BlockActionResult::SuccessServer
    }
}
