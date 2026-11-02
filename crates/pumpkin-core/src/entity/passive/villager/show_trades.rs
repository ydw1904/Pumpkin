use std::sync::Arc;

use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::entity::EntityType;
use pumpkin_data::environment_attribute::Activity;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;

use super::VillagerEntity;
use crate::entity::EntityBase;
use crate::entity::mob::Mob;
use crate::entity::mob::equipment::DEFAULT_EQUIPMENT_DROP_CHANCE;
use crate::world::World;

/// Vanilla `ShowTradesToPlayer(400, 1600)`, the behavior's duration bounds.
const MIN_DURATION: i64 = 400;
const MAX_DURATION: i64 = 1600;
/// Vanilla `ShowTradesToPlayer.MAX_LOOK_TIME` and `STARTING_LOOK_TIME`.
const MAX_LOOK_TIME: i32 = 900;
const STARTING_LOOK_TIME: i32 = 40;
/// Vanilla `ShowTradesToPlayer.displayCyclingItems`: ticks each offer is shown.
const CYCLE_TICKS: i32 = 40;
/// Vanilla `ShowTradesToPlayer.checkExtraStartConditions`: how close the player must stay.
const MAX_SHOW_DISTANCE_SQR: f64 = 17.0;
/// Vanilla `SetLookAndInteract.create(EntityTypes.PLAYER, 4)`.
const INTERACTION_RANGE_SQR: f64 = 4.0 * 4.0;

/// A running vanilla `ShowTradesToPlayer`.
pub(super) struct ShowTrades {
    /// The activity whose behavior is running; WORK, IDLE and MEET each have their own.
    activity: Activity,
    end_time: i64,
    player_item: Option<&'static Item>,
    display_items: Vec<ItemStack>,
    cycle_counter: i32,
    display_index: usize,
    look_time: i32,
}

impl VillagerEntity {
    /// The trade showing parts of the villager's WORK, IDLE and MEET packages: vanilla
    /// `SetLookAndInteract` for players, then `ShowTradesToPlayer`, which holds up what the
    /// player could buy with the item in their hand.
    pub(super) fn tick_show_trades(&self, world: &World, game_time: i64, activity: Activity) {
        let mut show_trades = self
            .show_trades
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let shows_trades = matches!(activity, Activity::Work | Activity::Idle | Activity::Meet);
        if show_trades
            .as_ref()
            .is_some_and(|running| running.activity != activity)
        {
            self.stop_showing_trades(&mut show_trades);
        }
        if !shows_trades {
            return;
        }
        if self.interaction_target(world).is_none() {
            self.set_look_and_interact(world);
        }

        let Some(running) = show_trades.as_mut() else {
            if let Some(player) = self.show_trades_target(world) {
                // Vanilla `Behavior.tryStart` picks the run's duration.
                let duration = MIN_DURATION
                    + i64::from(
                        self.get_random()
                            .random_range(0..=(MAX_DURATION - MIN_DURATION) as i32),
                    );
                self.look_at_player(&player);
                *show_trades = Some(ShowTrades {
                    activity,
                    end_time: game_time + duration,
                    player_item: None,
                    display_items: Vec::new(),
                    cycle_counter: 0,
                    display_index: 0,
                    look_time: STARTING_LOOK_TIME,
                });
            }
            return;
        };

        let player = self.show_trades_target(world);
        let Some(player) =
            player.filter(|_| game_time <= running.end_time && running.look_time > 0)
        else {
            self.stop_showing_trades(&mut show_trades);
            return;
        };
        self.look_at_player(&player);
        self.find_items_to_display(running, &player);
        if running.display_items.is_empty() {
            self.clear_held_item();
            running.look_time = running.look_time.min(STARTING_LOOK_TIME);
        } else if running.display_items.len() >= 2 {
            running.cycle_counter += 1;
            if running.cycle_counter >= CYCLE_TICKS {
                running.display_index = (running.display_index + 1) % running.display_items.len();
                running.cycle_counter = 0;
                self.display_as_held_item(running.display_items[running.display_index].clone());
            }
        }
        running.look_time -= 1;
    }

    /// Vanilla `SetLookAndInteract`: the closest visible player within 4 blocks becomes the
    /// interaction target.
    fn set_look_and_interact(&self, world: &World) {
        let entity = self.get_entity();
        let pos = entity.pos.load();
        let range = INTERACTION_RANGE_SQR.sqrt();
        let closest = world
            .get_players_at_box(&entity.bounding_box.load().expand(range, range, range))
            .into_iter()
            .filter(|player| {
                player.get_entity().is_alive()
                    && !player.is_spectator()
                    && player.get_entity().pos.load().squared_distance_to_vec(&pos)
                        <= INTERACTION_RANGE_SQR
                    && self.has_line_of_sight(player.get_entity())
            })
            .min_by(|a, b| {
                a.get_entity()
                    .pos
                    .load()
                    .squared_distance_to_vec(&pos)
                    .total_cmp(&b.get_entity().pos.load().squared_distance_to_vec(&pos))
            });
        if let Some(player) = closest {
            self.set_interaction_target(Some(player.entity_id()));
        }
    }

    /// Vanilla `ShowTradesToPlayer.checkExtraStartConditions`: the interaction target is a
    /// living player near this adult villager.
    fn show_trades_target(&self, world: &World) -> Option<Arc<dyn EntityBase>> {
        let entity = self.get_entity();
        let target = self.interaction_target(world)?;
        (target.get_entity().entity_type == &EntityType::PLAYER
            && self.mob_entity.living_entity.is_alive()
            && target
                .get_living_entity()
                .is_some_and(crate::entity::living::LivingEntity::is_alive)
            && entity.age.load(std::sync::atomic::Ordering::Relaxed) >= 0
            && target
                .get_entity()
                .pos
                .load()
                .squared_distance_to_vec(&entity.pos.load())
                <= MAX_SHOW_DISTANCE_SQR)
            .then_some(target)
    }

    /// Vanilla `ShowTradesToPlayer.stop`.
    fn stop_showing_trades(&self, show_trades: &mut Option<ShowTrades>) {
        *show_trades = None;
        self.set_interaction_target(None);
        self.clear_held_item();
    }

    /// Vanilla `ShowTradesToPlayer.findItemsToDisplay`: when the player's held item changes,
    /// collects the outputs of the offers in stock that it pays for.
    fn find_items_to_display(&self, running: &mut ShowTrades, player: &Arc<dyn EntityBase>) {
        let held = player
            .get_player()
            .map_or(&Item::AIR, |player| player.inventory.held_item().item);
        if running.player_item == Some(held) {
            return;
        }
        running.player_item = Some(held);
        running.display_items.clear();
        if held == &Item::AIR {
            return;
        }
        self.ensure_offers();
        running.display_items = self
            .offers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter(|offer| {
                !offer.is_out_of_stock()
                    && (offer.base_cost_a.0.item == held
                        || offer
                            .cost_b
                            .as_ref()
                            .is_some_and(|cost| cost.0.item == held))
            })
            .map(|offer| offer.output.0.as_ref().clone())
            .collect();
        if let Some(first) = running.display_items.first() {
            running.look_time = MAX_LOOK_TIME;
            self.display_as_held_item(first.clone());
        }
    }

    fn look_at_player(&self, player: &Arc<dyn EntityBase>) {
        self.mob_entity
            .look_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .look_at_entity(self, player);
    }

    /// Vanilla `ShowTradesToPlayer.clearHeldItem`.
    fn clear_held_item(&self) {
        let holding = !self
            .mob_entity
            .living_entity
            .entity_equipment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&EquipmentSlot::MAIN_HAND)
            .is_empty();
        if holding {
            self.mob_entity
                .set_item_slot(&EquipmentSlot::MAIN_HAND, ItemStack::EMPTY.clone());
        }
        self.set_main_hand_drop_chance(DEFAULT_EQUIPMENT_DROP_CHANCE);
    }

    /// Vanilla `ShowTradesToPlayer.displayAsHeldItem`.
    fn display_as_held_item(&self, stack: ItemStack) {
        self.mob_entity
            .set_item_slot(&EquipmentSlot::MAIN_HAND, stack);
        self.set_main_hand_drop_chance(0.0);
    }

    fn set_main_hand_drop_chance(&self, chance: f32) {
        self.mob_entity
            .living_entity
            .equipment_drop_chances
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(EquipmentSlot::MAIN_HAND, chance);
    }
}
