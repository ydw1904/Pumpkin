use std::sync::Arc;
use std::sync::Mutex;

use crate::block::{
    GetComparatorOutputArgs, GetScreenHandlerFactoryArgs, OnPlaceArgs, OnSyncedBlockEventArgs,
    PlacedArgs,
};
use crate::block::{
    registry::BlockActionResult,
    {BlockBehaviour, NormalUseArgs},
};

use crate::block::entities::shulker_box::ShulkerBoxBlockEntity;
use crate::world::World;
use pumpkin_data::block_properties::ShulkerBoxProperties;
use pumpkin_data::translation;
use pumpkin_data::{BlockStateId, FacingExt};
use pumpkin_inventory::Inventory;
use pumpkin_inventory::generic_container_screen_handler::create_shulker_box;
use pumpkin_inventory::player::player_inventory::PlayerInventory;
use pumpkin_inventory::screen_handler::{
    InventoryPlayer, ScreenHandlerFactory, SharedScreenHandler,
};
use pumpkin_macros::pumpkin_block_from_tag;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::text::TextComponent;
use std::sync::atomic::Ordering;

struct ShulkerBoxScreenFactory(Arc<dyn Inventory>);

impl ScreenHandlerFactory for ShulkerBoxScreenFactory {
    fn create_screen_handler(
        &self,
        sync_id: u8,
        player_inventory: &Arc<PlayerInventory>,
        player: &dyn InventoryPlayer,
    ) -> Option<SharedScreenHandler> {
        let handler = create_shulker_box(sync_id, player_inventory, self.0.clone(), player);
        let screen_handler_arc = Arc::new(Mutex::new(handler));

        Some(screen_handler_arc as SharedScreenHandler)
    }

    fn get_display_name(&self) -> TextComponent {
        pumpkin_macros::translate_cross!(
            translation::java::CONTAINER_SHULKERBOX,
            translation::bedrock::CONTAINER_SHULKERBOX
        )
    }
}

#[pumpkin_block_from_tag("minecraft:shulker_boxes")]
pub struct ShulkerBoxBlock;

type EndRodLikeProperties = pumpkin_data::block_properties::EndRodLikeProperties;

impl BlockBehaviour for ShulkerBoxBlock {
    fn on_place(&self, args: OnPlaceArgs<'_>) -> BlockStateId {
        let mut props = EndRodLikeProperties::default(args.block);
        props.facing = args.direction.to_facing().opposite();
        props.to_state_id(args.block)
    }

    fn on_synced_block_event(&self, args: OnSyncedBlockEventArgs<'_>) -> bool {
        // On the server, we don't need the Animation steps for now, because the client is responsible for that.
        // TODO: Do not open the shulker box when it is currently closing
        args.r#type == Self::OPEN_ANIMATION_EVENT_TYPE
    }

    fn placed(&self, args: PlacedArgs<'_>) {
        {
            let barrel_block_entity = ShulkerBoxBlockEntity::new(*args.position);
            args.world.add_block_entity(Arc::new(barrel_block_entity));
        }
    }

    fn normal_use(&self, args: NormalUseArgs<'_>) -> BlockActionResult {
        if !can_open(args.world, args.position) {
            return BlockActionResult::Success;
        }
        if let Some(factory) = self.get_screen_handler_factory(GetScreenHandlerFactoryArgs {
            server: args.server,
            world: args.world,
            block: args.block,
            position: args.position,
            player: args.player,
        }) {
            args.player.increment_stat(
                pumpkin_data::statistic::StatisticCategory::Custom,
                pumpkin_data::statistic::CustomStatistic::OpenShulkerBox as i32,
                1,
            );
            args.player
                .open_handled_screen(factory.as_ref(), Some(*args.position));
        }

        BlockActionResult::Success
    }

    fn get_screen_handler_factory(
        &self,
        args: GetScreenHandlerFactoryArgs<'_>,
    ) -> Option<Box<dyn ScreenHandlerFactory>> {
        let block_entity = args.world.get_block_entity(args.position)?;
        if !crate::block::entities::container_loot::open_container(
            &block_entity,
            args.world,
            args.player,
        ) {
            return None;
        }
        let inventory = block_entity.get_inventory()?;
        Some(Box::new(ShulkerBoxScreenFactory(inventory)))
    }

    fn get_comparator_output(&self, args: GetComparatorOutputArgs<'_>) -> Option<u8> {
        crate::block::container_comparator_output(&args)
    }
}

impl ShulkerBoxBlock {
    pub const OPEN_ANIMATION_EVENT_TYPE: u8 = 1;
}

/// `ShulkerBoxBlock.canOpen`: a closed box only opens when its lid has room to rise half a block.
fn can_open(world: &Arc<World>, position: &BlockPos) -> bool {
    let open = world.get_block_entity(position).is_some_and(|entity| {
        entity
            .as_any()
            .downcast_ref::<ShulkerBoxBlockEntity>()
            .is_some_and(|shulker| shulker.viewers.current.load(Ordering::Relaxed) > 0)
    });
    if open {
        return true;
    }
    let facing = ShulkerBoxProperties::from_state_id(world.get_block_state_id(position))
        .facing
        .to_block_direction()
        .to_offset();
    // Shulker.getProgressDeltaAabb(1, facing, 0, 0.5): the half block in front of the lid.
    let axis = |step: i32, low: f64, high: f64| match step {
        1 => (low + 1.0, high + 0.5),
        -1 => (low - 0.5, high - 1.0),
        _ => (low, high),
    };
    let (min_x, max_x) = axis(facing.x, -0.5, 0.5);
    let (min_y, max_y) = axis(facing.y, 0.0, 1.0);
    let (min_z, max_z) = axis(facing.z, -0.5, 0.5);
    let base = Vector3::new(
        f64::from(position.0.x) + 0.5,
        f64::from(position.0.y),
        f64::from(position.0.z) + 0.5,
    );
    let deflate = 1.0E-6;
    let lid = BoundingBox::new(
        base + Vector3::new(min_x + deflate, min_y + deflate, min_z + deflate),
        base + Vector3::new(max_x - deflate, max_y - deflate, max_z - deflate),
    );
    world.is_space_empty(lid)
}
