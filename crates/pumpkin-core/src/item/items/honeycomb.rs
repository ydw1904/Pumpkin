use std::sync::Arc;

use crate::block::UseWithItemArgs;
use crate::block::entities::BlockEntity;
use crate::block::registry::BlockActionResult;
use crate::entity::player::Player;
use crate::item::{ItemBehaviour, ItemMetadata};
use crate::server::Server;
use crate::world::World;
use pumpkin_data::Block;
use pumpkin_data::block_properties::{ChestLikeProperties, ChestType};
use pumpkin_data::block_transformer::get_waxed;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::world::WorldEvent;
use pumpkin_data::{BlockDirection, BlockId, BlockStateId};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::world::BlockFlags;

pub struct HoneyCombItem;

impl ItemMetadata for HoneyCombItem {
    fn ids() -> Box<[u16]> {
        [Item::HONEYCOMB.id].into()
    }
}

impl ItemBehaviour for HoneyCombItem {
    fn use_on_block(
        &self,
        item: &mut ItemStack,
        player: &Player,
        location: BlockPos,
        _face: BlockDirection,
        _cursor_pos: Vector3<f32>,
        block: &Block,
        _server: &Server,
    ) -> BlockActionResult {
        let world = player.world();
        let old_state_id = world.get_block_state_id(&location);
        if try_wax_block(&world, location, block) {
            after_player_wax(&world, location, old_state_id);
            item.decrement_unless_creative(player.gamemode.load(), 1);
            BlockActionResult::Success
        } else {
            BlockActionResult::Pass
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Waxes the block at `location` if it has a waxed equivalent, emitting the wax
/// particles and sound on success. This is all a dispenser does.
pub(crate) fn try_wax_block(world: &Arc<World>, location: BlockPos, block: &Block) -> bool {
    let Some(replacement) = get_waxed(block.id) else {
        return false;
    };
    let new_block = replacement.to_block();
    let current_state_id = world.get_block_state_id(&location);
    let new_state_id = new_block.with_properties_of(block, current_state_id);

    // Vanilla only sets the clicked block. The other door or chest half takes the waxed block
    // from it in its shape update.
    world.set_block_state(&location, new_state_id, BlockFlags::NOTIFY_ALL);
    world.sync_world_event(WorldEvent::ParticlesWaxOn, location, 0);
    world.play_sound(
        Sound::ItemHoneycombWaxOn,
        SoundCategory::Blocks,
        &location.to_f64(),
    );
    true
}

/// The rest of vanilla `HoneycombItem.useOn` after [`try_wax_block`]: the game events, and
/// the effects at the other half of a double copper chest.
fn after_player_wax(world: &Arc<World>, location: BlockPos, old_state_id: BlockStateId) {
    world.emit_game_event(GameEvent::BlockChange.name(), location.to_centered_f64());
    if !matches!(
        Block::from_state_id(old_state_id).id,
        BlockId::COPPER_CHEST
            | BlockId::EXPOSED_COPPER_CHEST
            | BlockId::WEATHERED_COPPER_CHEST
            | BlockId::OXIDIZED_COPPER_CHEST
    ) {
        return;
    }
    let chest_props = ChestLikeProperties::from_state_id(old_state_id);
    let neighbor_dir = match chest_props.r#type {
        ChestType::Left => chest_props.facing.rotate_clockwise(),
        ChestType::Right => chest_props.facing.rotate_counter_clockwise(),
        ChestType::Single => return,
    };
    let neighbor_pos = location.offset(neighbor_dir.to_offset());
    world.emit_game_event(
        GameEvent::BlockChange.name(),
        neighbor_pos.to_centered_f64(),
    );
    world.sync_world_event(WorldEvent::ParticlesWaxOn, neighbor_pos, 0);
    world.play_sound(
        Sound::ItemHoneycombWaxOn,
        SoundCategory::Blocks,
        &location.to_f64(),
    );
}

impl HoneyCombItem {
    pub fn apply_to_sign(
        &self,
        args: &UseWithItemArgs<'_>,
        block_entity: &Arc<dyn BlockEntity>,
        sign_entity: &crate::block::entities::sign::SignEntityRef<'_>,
    ) -> BlockActionResult {
        if sign_entity.is_waxed() {
            return BlockActionResult::PassToDefaultBlockAction;
        }
        sign_entity.set_waxed(true);

        args.world.update_block_entity(block_entity);
        args.world
            .sync_world_event(WorldEvent::ParticlesWaxOn, *args.position, 0);
        args.world.play_sound(
            Sound::ItemHoneycombWaxOn,
            SoundCategory::Blocks,
            &args.position.to_f64(),
        );

        BlockActionResult::Success
    }
}
