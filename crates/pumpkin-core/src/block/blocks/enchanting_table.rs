use std::sync::Arc;
use std::sync::Mutex;

use crate::block::entities::enchanting_table::EnchantingTableBlockEntity;
use crate::block::registry::BlockActionResult;
use crate::block::{
    BlockBehaviour, GetScreenHandlerFactoryArgs, NormalUseArgs, PathComputationType, PlacedArgs,
};
use pumpkin_data::{
    BlockState,
    tag::{self, Taggable},
    translation,
};
use pumpkin_inventory::enchanting::enchanting_screen_handler::EnchantingTableScreenHandler;
use pumpkin_inventory::player::player_inventory::PlayerInventory;
use pumpkin_inventory::screen_handler::{
    InventoryPlayer, ScreenHandlerFactory, SharedScreenHandler,
};
use pumpkin_inventory::{Inventory, SimpleInventory};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::text::TextComponent;

#[pumpkin_block("minecraft:enchanting_table")]
pub struct EnchantingTableBlock;

impl BlockBehaviour for EnchantingTableBlock {
    fn placed(&self, args: PlacedArgs<'_>) {
        let entity = EnchantingTableBlockEntity::new(*args.position);
        args.world.add_block_entity(Arc::new(entity));
    }

    fn normal_use(&self, args: NormalUseArgs<'_>) -> BlockActionResult {
        if let Some(factory) = self.get_screen_handler_factory(GetScreenHandlerFactoryArgs {
            server: args.server,
            world: args.world,
            block: args.block,
            position: args.position,
            player: args.player,
        }) {
            args.player
                .open_handled_screen(factory.as_ref(), Some(*args.position));
        }
        BlockActionResult::Success
    }

    fn get_screen_handler_factory(
        &self,
        args: GetScreenHandlerFactoryArgs<'_>,
    ) -> Option<Box<dyn ScreenHandlerFactory>> {
        let seed = args.player.enchantment_seed();
        let custom_name = args
            .world
            .get_block_entity(args.position)
            .and_then(|block_entity| block_entity.custom_name());
        Some(crate::block::entities::base_container::with_custom_name(
            custom_name,
            Box::new(EnchantingTableScreenFactory { seed }),
        ))
    }

    fn is_pathfindable(&self, _state: &BlockState, _computation_type: PathComputationType) -> bool {
        false
    }
}

impl EnchantingTableBlock {
    /// Vanilla `EnchantingTableBlock.BOOKSHELF_OFFSETS` filtered by `isValidBookShelf`: the
    /// power providers two blocks out whose halfway block is a power transmitter.
    pub fn count_bookshelves(world: &crate::world::World, pos: &BlockPos) -> i32 {
        let mut count = 0;
        for dy in 0..=1 {
            for dx in -2..=2i32 {
                for dz in -2..=2i32 {
                    if dx.abs() != 2 && dz.abs() != 2 {
                        continue;
                    }
                    if world
                        .get_block(&pos.add(dx, dy, dz))
                        .has_tag(&tag::Block::MINECRAFT_ENCHANTMENT_POWER_PROVIDER)
                        && world
                            .get_block(&pos.add(dx / 2, dy, dz / 2))
                            .has_tag(&tag::Block::MINECRAFT_ENCHANTMENT_POWER_TRANSMITTER)
                    {
                        count += 1;
                    }
                }
            }
        }
        count
    }
}

struct EnchantingTableScreenFactory {
    seed: i32,
}

impl ScreenHandlerFactory for EnchantingTableScreenFactory {
    fn create_screen_handler(
        &self,
        sync_id: u8,
        player_inventory: &Arc<PlayerInventory>,
        _player: &dyn InventoryPlayer,
    ) -> Option<SharedScreenHandler> {
        let inventory: Arc<dyn Inventory> = Arc::new(SimpleInventory::new(2));
        let handler =
            EnchantingTableScreenHandler::new(sync_id, player_inventory, &inventory, self.seed);
        let screen_handler_arc = Arc::new(Mutex::new(handler));
        Some(screen_handler_arc as SharedScreenHandler)
    }

    fn get_display_name(&self) -> TextComponent {
        pumpkin_macros::translate_cross!(
            translation::java::CONTAINER_ENCHANT,
            translation::bedrock::CONTAINER_ENCHANT
        )
    }
}
