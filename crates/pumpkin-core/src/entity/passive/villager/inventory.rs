use std::sync::Arc;
use std::sync::atomic::Ordering;

use pumpkin_data::data_component_impl::VillagerFoodImpl;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_util::math::position::BlockPos;

use super::VillagerEntity;
use super::meet::{WalkDestination, WalkTarget};
use crate::entity::EntityBase;
use crate::entity::item::ItemEntity;
use crate::entity::mob::Mob;
use crate::entity::player::advancement::trigger::AdvancementTrigger;
use crate::world::World;

/// Vanilla `Mob.ITEM_PICKUP_REACH`.
const ITEM_PICKUP_REACH: (f64, f64, f64) = (1.0, 0.0, 1.0);
/// Vanilla `NearestItemSensor`'s search box and `MAX_DISTANCE_TO_WANTED_ITEM`.
const NEAREST_ITEM_XZ_RANGE: f64 = 32.0;
const NEAREST_ITEM_Y_RANGE: f64 = 16.0;
const MAX_DISTANCE_TO_WANTED_ITEM: f64 = 32.0;
/// The `GoToWantedItem.create(speedModifier, false, 4)` of vanilla's villager core package.
const GO_TO_WANTED_ITEM_MAX_DISTANCE: f64 = 4.0;
const GO_TO_WANTED_ITEM_SPEED: f64 = 0.5;

/// Vanilla `SimpleContainer.canAddItem`.
pub(super) fn can_add_item(inventory: &[ItemStack], stack: &ItemStack) -> bool {
    inventory.iter().any(|slot| {
        slot.is_empty()
            || (slot.are_items_and_components_equal(stack)
                && slot.item_count < slot.get_max_stack_size())
    })
}

/// Vanilla `SimpleContainer.addItem`: fills stacks of the same item first, then the first
/// empty slot, and leaves what doesn't fit in `stack`.
pub(super) fn add_item(inventory: &mut [ItemStack], stack: &mut ItemStack) {
    for slot in inventory.iter_mut() {
        if stack.is_empty() {
            return;
        }
        if !slot.is_empty() && slot.are_items_and_components_equal(stack) {
            let moved = stack
                .item_count
                .min(slot.get_max_stack_size().saturating_sub(slot.item_count));
            slot.increment(moved);
            stack.decrement(moved);
        }
    }
    if stack.is_empty() {
        return;
    }
    if let Some(slot) = inventory.iter_mut().find(|slot| slot.is_empty()) {
        *slot = std::mem::replace(stack, ItemStack::EMPTY.clone());
    }
}

/// Vanilla `SimpleContainer.removeItemType`: takes up to `count` of `item` from the slots in
/// order and returns how many it took.
pub(super) fn remove_item_type(inventory: &mut [ItemStack], item: &Item, count: u8) -> u8 {
    let mut removed = 0;
    for slot in inventory.iter_mut() {
        if removed == count {
            break;
        }
        if slot.item == item {
            let taken = slot.item_count.min(count - removed);
            slot.decrement(taken);
            removed += taken;
            if slot.is_empty() {
                *slot = ItemStack::EMPTY.clone();
            }
        }
    }
    removed
}

impl VillagerEntity {
    /// Vanilla `Villager.wantsToPickUp`.
    fn wants_to_pick_up(&self, stack: &ItemStack) -> bool {
        (stack.item.has_tag(&tag::Item::MINECRAFT_VILLAGER_PICKS_UP)
            || stack.get_data_component::<VillagerFoodImpl>().is_some()
            || self.profession().requested_items().contains(&stack.item))
            && can_add_item(
                &self
                    .inventory
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
                stack,
            )
    }

    fn can_pick_up(&self, item: &ItemEntity) -> bool {
        let stack = item
            .get_item_stack()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        !item.get_entity().is_removed()
            && !stack.is_empty()
            && item.get_pickup_delay() == 0
            && self.wants_to_pick_up(&stack)
    }

    /// The item pickup of vanilla `Mob.aiStep`, with `Villager.pickUpItem` and
    /// `InventoryCarrier.pickUpItem`.
    pub(super) fn pick_up_items(&self, world: &World) {
        let living = &self.mob_entity.living_entity;
        if !self.mob_entity.can_pick_up_loot()
            || living.is_dead_or_dying()
            || !world.level_info.load().game_rules.mob_griefing
        {
            return;
        }
        let entity = self.get_entity();
        let (x, y, z) = ITEM_PICKUP_REACH;
        let area = entity.bounding_box.load().expand(x, y, z);
        for other in world.get_entities_at_box(&area) {
            let Some(item) = other.cast_any().downcast_ref::<ItemEntity>() else {
                continue;
            };
            if !self.can_pick_up(item) {
                continue;
            }
            let mut ground = item
                .get_item_stack()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let count = ground.item_count;
            let taken_stack = ground.clone();
            let mut remainder = ground.clone();
            add_item(
                &mut self
                    .inventory
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
                &mut remainder,
            );
            let taken = count - remainder.item_count;
            // Vanilla `LivingEntity.onItemPickup`.
            if let Some(thrower) = item
                .get_thrower()
                .and_then(|uuid| world.get_player_by_uuid(uuid))
            {
                thrower.trigger_advancement(AdvancementTrigger::ThrownItemPickedUpByEntity {
                    entity_type: "minecraft:villager".to_string(),
                    item_id: taken_stack.item.registry_key.to_string(),
                    is_baby: entity.age.load(Ordering::Relaxed) < 0,
                    player_wearing_safe_armor: false,
                });
            }
            living.pickup(item.get_entity(), u32::from(taken));
            if remainder.is_empty() {
                drop(ground);
                item.get_entity().remove();
            } else {
                ground.set_count(remainder.item_count);
                drop(ground);
                item.init_data_tracker();
            }
        }
    }

    /// Vanilla `NearestItemSensor`: the closest wanted item in sight within 32 blocks.
    fn nearest_visible_wanted_item(&self, world: &World) -> Option<Arc<dyn EntityBase>> {
        let entity = self.get_entity();
        let pos = entity.pos.load();
        let area = entity.bounding_box.load().expand(
            NEAREST_ITEM_XZ_RANGE,
            NEAREST_ITEM_Y_RANGE,
            NEAREST_ITEM_XZ_RANGE,
        );
        let mut items = world
            .get_entities_at_box(&area)
            .into_iter()
            .filter(|other| other.cast_any().is::<ItemEntity>())
            .collect::<Vec<_>>();
        items.sort_by(|a, b| {
            a.get_entity()
                .pos
                .load()
                .squared_distance_to_vec(&pos)
                .total_cmp(&b.get_entity().pos.load().squared_distance_to_vec(&pos))
        });
        items.into_iter().find(|other| {
            let Some(item) = other.cast_any().downcast_ref::<ItemEntity>() else {
                return false;
            };
            let stack = item
                .get_item_stack()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            self.wants_to_pick_up(&stack)
                && other.get_entity().pos.load().squared_distance_to_vec(&pos)
                    < MAX_DISTANCE_TO_WANTED_ITEM.powi(2)
                && self.has_line_of_sight(other.get_entity())
        })
    }

    /// Vanilla `GoToWantedItem` of the core package: walks to a wanted item within 4 blocks.
    /// The sensor behind it scans every 20 ticks, on the ticks `sensed` marks.
    pub(super) fn go_to_wanted_item(&self, world: &World, sensed: bool) {
        if sensed {
            *self
                .nearest_wanted_item
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = self
                .nearest_visible_wanted_item(world)
                .map(|item| item.get_entity().entity_id);
        }
        if self.walk_target().is_some() || !self.mob_entity.can_pick_up_loot() {
            return;
        }
        let Some(item) = (*self
            .nearest_wanted_item
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner))
        .and_then(|id| world.get_entity_by_id(id))
        .filter(|item| !item.get_entity().is_removed()) else {
            return;
        };
        let item_pos = item.get_entity().pos.load();
        let block = BlockPos::floored_v(item_pos);
        if item_pos.squared_distance_to_vec(&self.get_entity().pos.load())
            >= GO_TO_WANTED_ITEM_MAX_DISTANCE.powi(2)
            || !world
                .worldborder
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains_block(block.0.x, block.0.z)
        {
            return;
        }
        self.look_at_position(item.get_entity().pos.load());
        self.set_walk_target(Some(WalkTarget {
            destination: WalkDestination::Entity(item.get_entity().entity_id),
            speed: GO_TO_WANTED_ITEM_SPEED,
            close_enough_dist: 0,
        }));
    }
}
