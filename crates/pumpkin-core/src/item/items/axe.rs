use crate::block::registry::BlockActionResult;
use crate::entity::player::Player;
use crate::item::{ItemBehaviour, ItemMetadata};
use crate::server::Server;
use pumpkin_data::block_properties::{ChestLikeProperties, ChestType};
use pumpkin_data::data_component_impl::BlockTransformerImpl;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::SoundCategory;
use pumpkin_data::tag::Taggable;
use pumpkin_data::{Block, BlockDirection, tag};
use pumpkin_util::GameMode;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::world::BlockFlags;

pub struct AxeItem;

impl ItemMetadata for AxeItem {
    fn ids() -> Box<[u16]> {
        tag::Item::MINECRAFT_AXES.1.into()
    }
}

impl ItemBehaviour for AxeItem {
    fn use_on_block(
        &self,
        item: &mut ItemStack,
        player: &Player,
        location: BlockPos,
        face: BlockDirection,
        _cursor_pos: Vector3<f32>,
        block: &Block,
        _server: &Server,
    ) -> BlockActionResult {
        let Some(transformer) = item
            .get_data_component::<BlockTransformerImpl>()
            .and_then(BlockTransformerImpl::get_transformer)
        else {
            return BlockActionResult::Pass;
        };

        let world = player.world();
        let get_block = |dx: i8, dy: i8, dz: i8| {
            let check_pos = BlockPos(location.0 + Vector3::new(dx as i32, dy as i32, dz as i32));
            world.get_block(&check_pos)
        };

        let current_state_id = world.get_block_state_id(&location);

        if let Some(result) = transformer.transform(block, current_state_id, face, &get_block) {
            // Vanilla `BlockTransformer.transformBlock` only sets the clicked block. The other
            // door or chest half takes the new block from it in its shape update.
            world.set_block_state(&location, result.new_state_id, BlockFlags::NOTIFY_ALL);
            if let Some(sound) = result.entry.sound {
                world.play_sound(sound, SoundCategory::Blocks, &location.to_f64());
            }
            if let Some(particle) = result.entry.particle {
                world.sync_world_event(particle, location, 0);
            }
            let game_event_context = crate::world::game_event::GameEventContext {
                source_entity: Some(player),
                affected_state: None,
            };
            world.emit_game_event_with_context(
                GameEvent::BlockChange.name(),
                location.to_centered_f64(),
                &game_event_context,
            );
            if block.has_tag(&tag::Block::MINECRAFT_COPPER_CHESTS) {
                let chest_props = ChestLikeProperties::from_state_id(current_state_id);
                let neighbor_dir = match chest_props.r#type {
                    ChestType::Left => Some(chest_props.facing.rotate_clockwise()),
                    ChestType::Right => Some(chest_props.facing.rotate_counter_clockwise()),
                    ChestType::Single => None,
                };
                if let Some(neighbor_dir) = neighbor_dir {
                    let neighbor_pos = location.offset(neighbor_dir.to_offset());
                    world.emit_game_event_with_context(
                        GameEvent::BlockChange.name(),
                        neighbor_pos.to_centered_f64(),
                        &game_event_context,
                    );
                    if let Some(particle) = result.entry.particle {
                        world.sync_world_event(particle, neighbor_pos, 0);
                    }
                }
            }

            if player.gamemode.load() != GameMode::Creative {
                let _ = item.damage_item(i32::from(result.entry.item_damage_per_use));
            }
            return BlockActionResult::Success;
        }
        BlockActionResult::Pass
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
