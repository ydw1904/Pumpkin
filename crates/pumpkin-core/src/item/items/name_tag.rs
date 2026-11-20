use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::entity::EntityBase;
use crate::entity::player::Player;
use crate::item::{ItemBehaviour, ItemMetadata};
use pumpkin_data::data_component_impl::CustomNameImpl;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;

pub struct NameTagItem;

impl NameTagItem {
    /// Vanilla `NameTagItem.interactLivingEntity`. `Mob.interact` runs it before the mob's own
    /// interaction.
    pub fn interact_living_entity(
        item: &mut ItemStack,
        player: &Player,
        entity: &dyn EntityBase,
    ) -> bool {
        let raw_entity = entity.get_entity();
        let Some(living) = entity.get_living_entity() else {
            return false;
        };
        let Some(name) = item.get_data_component::<CustomNameImpl>() else {
            return false;
        };
        if !raw_entity.entity_type.saveable {
            return false;
        }
        // Vanilla succeeds on a dead target too, it just doesn't rename it.
        if living.is_alive() {
            raw_entity.set_custom_name(name.name.clone());
            if let Some(mob) = entity.get_mob() {
                mob.get_mob_entity()
                    .persistence_required
                    .store(true, Ordering::Relaxed);
            }
            item.decrement_unless_creative(player.gamemode.load(), 1);
        }
        true
    }
}

impl ItemMetadata for NameTagItem {
    fn ids() -> Box<[u16]> {
        [Item::NAME_TAG.id].into()
    }
}

impl ItemBehaviour for NameTagItem {
    fn use_on_entity(
        &self,
        item: &mut ItemStack,
        player: &Player,
        entity: Arc<dyn EntityBase>,
    ) -> bool {
        Self::interact_living_entity(item, player, entity.as_ref())
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
