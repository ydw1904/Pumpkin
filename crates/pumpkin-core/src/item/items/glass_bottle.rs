use std::any::Any;

use crate::entity::EntityBase;
use crate::entity::player::Player;
use crate::item::{ItemBehaviour, ItemMetadata};
use pumpkin_data::data_component::DataComponent;
use pumpkin_data::data_component_impl::PotionContentsImpl;
use pumpkin_data::entity::EntityType;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::potion::Potion;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::statistic::StatisticCategory;
use pumpkin_data::tag::{self, Taggable};

/// A potion holding water; the contents component is what names it and makes it
/// brewable.
#[must_use]
pub fn water_bottle() -> ItemStack {
    ItemStack::new_with_component(
        1,
        &Item::POTION,
        vec![(
            DataComponent::PotionContents,
            Some(Box::new(PotionContentsImpl {
                potion_id: Some(i32::from(Potion::WATER.id)),
                custom_color: None,
                custom_effects: Vec::new(),
                custom_name: None,
            }) as Box<_>),
        )],
    )
}

pub struct GlassBottleItem;

impl ItemMetadata for GlassBottleItem {
    fn ids() -> Box<[u16]> {
        Box::new([Item::GLASS_BOTTLE.id])
    }
}

impl GlassBottleItem {
    fn turn_bottle_into_item(
        player: &Player,
        hand: pumpkin_util::Hand,
        mut held: ItemStack,
        mut stack_to_give: ItemStack,
    ) {
        player.increment_stat(StatisticCategory::Used, i32::from(Item::GLASS_BOTTLE.id), 1);
        let world = player.world();
        if held.item_count == 1 && player.gamemode.load() != pumpkin_util::GameMode::Creative {
            match hand {
                pumpkin_util::Hand::Right => player.inventory().set_held_item(stack_to_give),
                pumpkin_util::Hand::Left => player
                    .inventory()
                    .set_stack_in_hand(pumpkin_util::Hand::Left, stack_to_give),
            }
        } else {
            held.decrement_unless_creative(player.gamemode.load(), 1);
            match hand {
                pumpkin_util::Hand::Right => player.inventory().set_held_item(held),
                pumpkin_util::Hand::Left => player
                    .inventory()
                    .set_stack_in_hand(pumpkin_util::Hand::Left, held),
            }
            let was_added = player.inventory().insert_stack_anywhere(&mut stack_to_give);
            if !was_added && !stack_to_give.is_empty() {
                world.drop_stack(&player.position().to_block_pos(), stack_to_give);
            }
        }
    }
}

impl ItemBehaviour for GlassBottleItem {
    fn normal_use(&self, item: &Item, player: &Player) {
        let (yaw, pitch) = player.rotation();
        self.normal_use_with_hand(item, player, yaw, pitch, pumpkin_util::Hand::Right);
    }

    fn normal_use_with_hand(
        &self,
        _item: &Item,
        player: &Player,
        yaw: f32,
        pitch: f32,
        hand: pumpkin_util::Hand,
    ) {
        let held = match hand {
            pumpkin_util::Hand::Right => player.inventory().held_item(),
            pumpkin_util::Hand::Left => player.inventory().off_hand_item(),
        };
        if held.is_empty() || held.item.id != Item::GLASS_BOTTLE.id {
            return;
        }

        let world = player.world();
        let search_box = player
            .get_entity()
            .bounding_box
            .load()
            .expand(2.0, 2.0, 2.0);
        for candidate in world.get_entities_at_box(&search_box) {
            if let Some(cloud) = candidate
                .cast_any()
                .downcast_ref::<crate::entity::area_effect_cloud::AreaEffectCloudEntity>(
            ) && cloud.entity.is_alive()
                && cloud.owner().is_some_and(|owner| {
                    owner.get_entity().entity_type == &EntityType::ENDER_DRAGON
                })
            {
                cloud.set_radius(cloud.radius() - 0.5);
                world.play_sound(
                    Sound::ItemBottleFillDragonbreath,
                    SoundCategory::Neutral,
                    &player.position(),
                );
                world.emit_game_event(GameEvent::FluidPickup.name(), player.position());
                Self::turn_bottle_into_item(
                    player,
                    hand,
                    held,
                    ItemStack::new(1, &Item::DRAGON_BREATH),
                );
                return;
            }
        }

        let (start_pos, end_pos) = self.get_start_and_end_pos_with_rotation(player, yaw, pitch);
        let hit = world.clip(&pumpkin_world::clip::ClipContext::new(
            start_pos,
            end_pos,
            pumpkin_world::clip::ClipBlock::Outline,
            pumpkin_world::clip::ClipFluid::SourceOnly,
        ));

        if !hit.is_miss {
            let hit_pos = hit.block_pos;
            if !world.may_interact(player, &hit_pos) {
                return;
            }
            if world
                .get_fluid(&hit_pos)
                .has_tag(&tag::Fluid::MINECRAFT_WATER)
            {
                // The client plays the fill sound itself.
                world.play_sound_expect(
                    player,
                    Sound::ItemBottleFill,
                    SoundCategory::Neutral,
                    &player.position(),
                );
                world.emit_game_event(GameEvent::FluidPickup.name(), hit_pos.to_f64());
                Self::turn_bottle_into_item(player, hand, held, water_bottle());
            }
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
