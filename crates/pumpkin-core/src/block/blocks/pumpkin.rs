use crate::block::UseWithItemArgs;
use crate::block::registry::BlockActionResult;
use crate::entity::Entity;
use crate::entity::item::ItemEntity;
use pumpkin_data::Block;
use pumpkin_data::block_properties::WallTorchLikeProperties;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_macros::pumpkin_block;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::random::gameplay::game_random;
use pumpkin_world::world::BlockFlags;
use std::sync::Arc;

#[pumpkin_block("minecraft:pumpkin")]
pub struct PumpkinBlock;

impl crate::block::BlockBehaviour for PumpkinBlock {
    fn use_with_item(&self, args: UseWithItemArgs<'_>) -> BlockActionResult {
        if args.item_stack.item != &Item::SHEARS {
            return BlockActionResult::PassToDefaultBlockAction;
        }
        let mut props = WallTorchLikeProperties::default(&Block::CARVED_PUMPKIN);
        props.facing = args.hit.face.to_horizontal_facing().unwrap_or_else(|| {
            args.player
                .living_entity
                .entity
                .get_horizontal_facing()
                .opposite()
        });
        args.world.play_sound(
            pumpkin_data::sound::Sound::BlockPumpkinCarve,
            pumpkin_data::sound::SoundCategory::Blocks,
            &args.position.to_f64(),
        );
        args.world.set_block_state(
            args.position,
            props.to_state_id(&Block::CARVED_PUMPKIN),
            BlockFlags::NOTIFY_ALL,
        );
        let drop_pos = args.position.to_centered_f64();
        let drops = args
            .world
            .get_loot_table("minecraft:carve/pumpkin")
            .map_or_else(
                || vec![ItemStack::new(4, &Item::PUMPKIN_SEEDS)],
                |handle| {
                    let params = crate::world::loot::LootContextParameters {
                        world: Some(args.world),
                        position: Some(drop_pos),
                        this_entity: Some(args.player.as_ref() as &dyn crate::entity::EntityBase),
                        tool: Some(args.item_stack.clone()),
                        ..Default::default()
                    };
                    crate::world::loot::generate_loot_from_handle(&handle, rand::random(), &params)
                },
            );
        // Vanilla throws the seeds out of the carved face.
        let step = props.facing.to_offset();
        let (step_x, step_z) = (f64::from(step.x), f64::from(step.z));
        let spawn_pos = Vector3::new(
            f64::from(args.position.0.x) + 0.5 + step_x * 0.65,
            f64::from(args.position.0.y) + 0.1,
            f64::from(args.position.0.z) + 0.5 + step_z * 0.65,
        );
        for drop_stack in drops {
            let velocity = {
                let rng = game_random();
                Vector3::new(
                    0.05 * step_x + rng.next_f64() * 0.02,
                    0.05,
                    0.05 * step_z + rng.next_f64() * 0.02,
                )
            };
            let entity = Entity::new(args.world.clone(), spawn_pos, &EntityType::ITEM);
            let item_entity = Arc::new(ItemEntity::new(entity, drop_stack));
            item_entity.get_entity().velocity.store(velocity);
            args.world.spawn_entity(item_entity);
        }
        args.player.damage_item_in_slot(args.equipment_slot, 1);
        args.world.emit_game_event_with_context(
            pumpkin_data::game_event::GameEvent::Shear.name(),
            args.position.to_centered_f64(),
            &crate::world::game_event::GameEventContext {
                source_entity: Some(args.player.as_ref()),
                affected_state: None,
            },
        );
        args.player.increment_stat(
            pumpkin_data::statistic::StatisticCategory::Used,
            i32::from(Item::SHEARS.id),
            1,
        );
        BlockActionResult::Success
    }
}
