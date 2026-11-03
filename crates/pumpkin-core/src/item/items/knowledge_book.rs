use std::any::Any;

use crate::entity::player::Player;
use crate::entity::player::recipe_book::is_learnable_recipe;
use crate::item::{ItemBehaviour, ItemMetadata};
use pumpkin_data::data_component_impl::{CustomDataImpl, RecipesImpl};
use pumpkin_data::item::Item;
use pumpkin_data::statistic::StatisticCategory;
use pumpkin_inventory::player::player_inventory::PlayerInventory;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_util::Hand;

pub struct KnowledgeBookItem;
pub struct DiscFragmentItem;

impl ItemMetadata for KnowledgeBookItem {
    fn ids() -> Box<[u16]> {
        Box::new([Item::KNOWLEDGE_BOOK.id])
    }
}

impl ItemBehaviour for KnowledgeBookItem {
    fn normal_use(&self, item: &Item, player: &Player) {
        let (yaw, pitch) = player.rotation();
        self.normal_use_with_hand(item, player, yaw, pitch, Hand::Right);
    }

    fn normal_use_with_hand(
        &self,
        _item: &Item,
        player: &Player,
        _yaw: f32,
        _pitch: f32,
        hand: Hand,
    ) {
        let mut held = player.inventory().get_stack_in_hand(hand);
        if held.is_empty() || held.item.id != Item::KNOWLEDGE_BOOK.id {
            return;
        }

        let mut recipe_ids: Vec<String> = held
            .get_data_component::<RecipesImpl>()
            .map(|r| r.recipes.iter().map(ToString::to_string).collect())
            .unwrap_or_default();

        if recipe_ids.is_empty()
            && let Some(custom) = held.get_data_component::<CustomDataImpl>()
            && let Some(list) = custom
                .data
                .get_list("Recipes")
                .or_else(|| custom.data.get_list("recipes"))
        {
            for tag in list {
                if let NbtTag::String(s) = tag {
                    recipe_ids.push(s.to_string());
                }
            }
        }

        held.decrement_unless_creative(player.gamemode.load(), 1);
        player.inventory().set_stack_in_hand(hand, held.clone());
        let slot_index = match hand {
            Hand::Right => player.inventory().get_selected_slot() as usize,
            Hand::Left => PlayerInventory::OFF_HAND_SLOT,
        };
        player.sync_hand_slot(slot_index, held);

        if recipe_ids.is_empty() {
            return;
        }

        let Some(server) = player.world().server.upgrade() else {
            return;
        };
        let dynamic_recipes = server.recipe_manager.get_dynamic_recipes_internal();
        let recipe_ids: Vec<String> = recipe_ids
            .into_iter()
            .map(|id| {
                if id.contains(':') {
                    id
                } else {
                    format!("minecraft:{id}")
                }
            })
            .collect();
        if let Some(invalid) = recipe_ids
            .iter()
            .find(|id| !is_learnable_recipe(id, &dynamic_recipes))
        {
            tracing::error!("Invalid recipe: {invalid}");
            return;
        }

        player.award_recipes(&recipe_ids);
        player.increment_stat(
            StatisticCategory::Used,
            i32::from(Item::KNOWLEDGE_BOOK.id),
            1,
        );
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl ItemMetadata for DiscFragmentItem {
    fn ids() -> Box<[u16]> {
        Box::new([Item::DISC_FRAGMENT_5.id])
    }
}

impl ItemBehaviour for DiscFragmentItem {
    fn as_any(&self) -> &dyn Any {
        self
    }
}
