use std::sync::Arc;
use std::sync::atomic::Ordering;

use pumpkin_data::data_component_impl::VillagerFoodImpl;
use pumpkin_data::entity::EntityType;
use pumpkin_data::environment_attribute::Activity;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::villager::VillagerProfession;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

use super::VillagerEntity;
use crate::entity::EntityBase;
use crate::entity::ai::brain::behavior::utils::throw_item;
use crate::entity::ai::goal::{Controls, Goal};
use crate::entity::ai::util::land_random_pos;
use crate::entity::mob::Mob;
use crate::world::World;

/// Vanilla `StrollAroundPoi.MIN_TIME_BETWEEN_STROLLS`.
const MIN_TIME_BETWEEN_STROLLS: i64 = 180;
/// Vanilla `StrollAroundPoi.STROLL_MAX_XZ_DIST`.
const STROLL_MAX_XZ_DIST: i32 = 8;
/// Vanilla `StrollAroundPoi.STROLL_MAX_Y_DIST`.
const STROLL_MAX_Y_DIST: i32 = 6;
/// The `StrollAroundPoi.create(MEETING_POINT, 0.4F, 40)` of vanilla's meet package.
const STROLL_SPEED: f64 = 0.4;
const STROLL_MAX_DISTANCE_FROM_POI: f64 = 40.0;
/// Vanilla `SocializeAtBell.SPEED_MODIFIER`.
const SOCIALIZE_SPEED: f64 = 0.3;
/// Vanilla `VillagerGoalPackages.INTERACT_DIST_SQR`.
const INTERACT_DIST_SQR: f64 = 5.0;
/// Vanilla `VillagerGoalPackages.INTERACT_WALKUP_DIST`.
const INTERACT_WALKUP_DIST: i32 = 2;
/// Vanilla `VillagerGoalPackages.INTERACT_SPEED_MODIFIER`.
const INTERACT_SPEED: f64 = 0.5;
/// Vanilla `Villager.hasExcessFood` and `wantsMoreFood` thresholds, in food points.
const EXCESS_FOOD_POINTS: i32 = 24;
const WANTS_MORE_FOOD_POINTS: i32 = 12;
/// Vanilla `Villager.gossip`: ticks between two gossips of the same villager.
const GOSSIP_INTERVAL: i64 = 1200;
/// Vanilla `Villager.gossip`'s `transferFrom(..., 10)`.
const GOSSIP_TRANSFER_COUNT: usize = 10;
/// Vanilla `GossipContainer.transferFrom` drops gossips weaker than this.
const MIN_TRANSFERRED_GOSSIP: i32 = 2;
/// Vanilla `Villager.gossip`'s `spawnGolemIfNeeded(level, timestamp, 5)`.
const GOSSIP_VILLAGERS_TO_AGREE: usize = 5;
/// `Behavior`'s default duration, which `TradeWithVillager` keeps.
const TRADE_WITH_VILLAGER_DURATION: i64 = 60;
/// Vanilla `TradeWithVillager.throwHalfStack` keeps this many of a small stack.
const THROW_KEEP_COUNT: u8 = 24;

#[derive(Clone, Copy, PartialEq)]
pub enum WalkDestination {
    Pos(Vector3<f64>),
    Entity(i32),
}

/// Vanilla `WalkTarget`, which `MoveToTargetSink` follows.
#[derive(Clone, Copy, PartialEq)]
pub struct WalkTarget {
    pub destination: WalkDestination,
    pub speed: f64,
    pub close_enough_dist: i32,
}

impl WalkTarget {
    fn position(&self, world: &World) -> Option<Vector3<f64>> {
        match self.destination {
            WalkDestination::Pos(pos) => Some(pos),
            WalkDestination::Entity(id) => world
                .get_entity_by_id(id)
                .map(|entity| entity.get_entity().pos.load()),
        }
    }
}

const fn manhattan(a: BlockPos, b: BlockPos) -> i32 {
    (a.0.x - b.0.x).abs() + (a.0.y - b.0.y).abs() + (a.0.z - b.0.z).abs()
}

impl VillagerEntity {
    pub(super) fn set_walk_target(&self, target: Option<WalkTarget>) {
        *self
            .walk_target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = target;
    }

    pub(super) fn walk_target(&self) -> Option<WalkTarget> {
        *self
            .walk_target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(super) fn interaction_target(
        &self,
        world: &World,
    ) -> Option<std::sync::Arc<dyn EntityBase>> {
        let id = (*self
            .interaction_target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner))?;
        world.get_entity_by_id(id)
    }

    pub(super) fn set_interaction_target(&self, target: Option<i32>) {
        *self
            .interaction_target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = target;
    }

    /// The `MEET` activity of vanilla `VillagerGoalPackages.getMeetPackage` besides walking to
    /// the bell: `TriggerGate.triggerOneShuffled` of `StrollAroundPoi` and `SocializeAtBell`,
    /// then the `TradeWithVillager` gate on the interaction target.
    pub(super) fn tick_meet(&self, world: &Arc<World>, game_time: i64, activity: Activity) {
        if activity == Activity::Meet {
            self.meet_at_meeting_point(world, game_time);
        }
        // The trade gate of the MEET and IDLE packages. Like any behavior, a started trade
        // runs out its time after the activity changes.
        if matches!(activity, Activity::Meet | Activity::Idle)
            || self.trade_with_villager_end.load(Ordering::Relaxed) != 0
        {
            self.trade_with_villager(world, game_time);
        }
    }

    fn meet_at_meeting_point(&self, world: &Arc<World>, game_time: i64) {
        let Some(meeting_point) = self.get_meeting_point() else {
            return;
        };

        let random = self.get_random();
        let mut next_ok_start = self.stroll_next_ok_start.load(Ordering::Relaxed);
        let mut stroll = || {
            self.stroll_around_poi(
                meeting_point,
                STROLL_MAX_DISTANCE_FROM_POI,
                &mut next_ok_start,
                game_time,
            )
        };
        if random.random_bool(0.5) {
            if !self.socialize_at_bell(world, meeting_point) {
                stroll();
            }
        } else if !stroll() {
            self.socialize_at_bell(world, meeting_point);
        }
        self.stroll_next_ok_start
            .store(next_ok_start, Ordering::Relaxed);
    }

    /// Vanilla `StrollAroundPoi`.
    pub(super) fn stroll_around_poi(
        &self,
        poi: BlockPos,
        max_distance_from_poi: f64,
        next_ok_start: &mut i64,
        game_time: i64,
    ) -> bool {
        let pos = self.get_entity().pos.load();
        if poi.to_centered_f64().squared_distance_to_vec(&pos) >= max_distance_from_poi.powi(2) {
            return false;
        }
        if game_time <= *next_ok_start {
            return true;
        }
        let land_pos = land_random_pos::get_pos(self, STROLL_MAX_XZ_DIST, STROLL_MAX_Y_DIST);
        self.set_walk_target(land_pos.map(|pos| WalkTarget {
            destination: WalkDestination::Pos(pos),
            speed: STROLL_SPEED,
            close_enough_dist: 1,
        }));
        *next_ok_start = game_time + MIN_TIME_BETWEEN_STROLLS;
        true
    }

    /// Vanilla `SocializeAtBell`.
    fn socialize_at_bell(&self, world: &World, meeting_point: BlockPos) -> bool {
        if self.interaction_target(world).is_some() {
            return false;
        }
        let pos = self.get_entity().pos.load();
        if self.get_random().random_range(0..100) != 0
            || meeting_point
                .to_centered_f64()
                .squared_distance_to_vec(&pos)
                >= 4.0f64.powi(2)
        {
            return false;
        }
        let villagers = self
            .nearest_living_entities(world)
            .into_iter()
            .filter(|other| other.get_entity().entity_type == &EntityType::VILLAGER)
            .collect::<Vec<_>>();
        if villagers.is_empty() {
            return false;
        }
        if let Some(villager) = villagers
            .iter()
            .find(|other| other.get_entity().pos.load().squared_distance_to_vec(&pos) <= 32.0)
        {
            let id = villager.get_entity().entity_id;
            self.set_interaction_target(Some(id));
            self.set_walk_target(Some(WalkTarget {
                destination: WalkDestination::Entity(id),
                speed: SOCIALIZE_SPEED,
                close_enough_dist: 1,
            }));
        }
        true
    }

    /// Vanilla `BehaviorUtils.lockGazeAndWalkToEachOther`.
    fn lock_gaze_and_walk_to_each_other(&self, other: &Self) {
        for (walker, other) in [(self, other), (other, self)] {
            walker.look_at_position(other.get_entity().get_eye_pos());
            walker.set_walk_target(Some(WalkTarget {
                destination: WalkDestination::Entity(other.get_entity().entity_id),
                speed: INTERACT_SPEED,
                close_enough_dist: INTERACT_WALKUP_DIST,
            }));
        }
    }

    /// Vanilla `TradeWithVillager` behind its gate: for its 60 ticks, while the interaction
    /// target is a living villager, both walk up to and look at each other, gossip, and share
    /// food and wanted items. Stopping erases the interaction target.
    fn trade_with_villager(&self, world: &Arc<World>, game_time: i64) {
        let end = self.trade_with_villager_end.load(Ordering::Relaxed);
        let target = self.interaction_target(world).filter(|target| {
            target
                .cast_any()
                .downcast_ref::<Self>()
                .is_some_and(|villager| villager.mob_entity.living_entity.is_alive())
        });
        let Some(target) = target.filter(|_| end == 0 || game_time <= end) else {
            if end != 0 {
                self.trade_with_villager_end.store(0, Ordering::Relaxed);
                self.set_interaction_target(None);
            }
            return;
        };
        let Some(villager) = target.cast_any().downcast_ref::<Self>() else {
            return;
        };
        if end == 0 {
            self.trade_with_villager_end
                .store(game_time + TRADE_WITH_VILLAGER_DURATION, Ordering::Relaxed);
            self.lock_gaze_and_walk_to_each_other(villager);
        }
        let pos = self.get_entity().pos.load();
        if target.get_entity().pos.load().squared_distance_to_vec(&pos) > INTERACT_DIST_SQR {
            return;
        }
        self.lock_gaze_and_walk_to_each_other(villager);

        self.gossip(world, villager, game_time);
        let profession = self.profession();
        let is_farmer = profession == VillagerProfession::Farmer;
        let target_pos = target.get_entity().pos.load();
        if self.count_food_points_in_inventory() >= EXCESS_FOOD_POINTS
            && (is_farmer || villager.count_food_points_in_inventory() < WANTS_MORE_FOOD_POINTS)
        {
            self.throw_half_stack(
                |stack| stack.get_data_component::<VillagerFoodImpl>().is_some(),
                target_pos,
            );
        }
        if is_farmer
            && self.count_item(&Item::WHEAT)
                > i32::from(ItemStack::new(1, &Item::WHEAT).get_max_stack_size()) / 2
        {
            self.throw_half_stack(|stack| stack.item == &Item::WHEAT, target_pos);
        }
        // Vanilla `figureOutWhatIAmWillingToTrade`: what the target asks for and this one doesn't.
        let own_requests = profession.requested_items();
        let trades = villager
            .profession()
            .requested_items()
            .iter()
            .filter(|item| !own_requests.contains(item))
            .collect::<Vec<_>>();
        if !trades.is_empty() {
            self.throw_half_stack(|stack| trades.contains(&&stack.item), target_pos);
        }
    }

    /// Vanilla `Villager.gossip`: at most once a minute for both, this villager hears up to ten
    /// of the target's gossips, weakened on the way.
    fn gossip(&self, world: &Arc<World>, target: &Self, game_time: i64) {
        let can_gossip = |villager: &Self| {
            let last = villager.last_gossip_share_time.load(Ordering::Relaxed);
            game_time < last || game_time >= last + GOSSIP_INTERVAL
        };
        if !can_gossip(self) || !can_gossip(target) {
            return;
        }
        let new_gossip = self.transfer_gossips_from(target);
        self.last_gossip_share_time
            .store(game_time, Ordering::Relaxed);
        target
            .last_gossip_share_time
            .store(game_time, Ordering::Relaxed);
        self.spawn_golem_if_needed(world, game_time, GOSSIP_VILLAGERS_TO_AGREE);
        if new_gossip > 0
            && let Some(player) = self.get_trading_player()
        {
            self.update_special_prices(&player);
        }
    }

    /// Vanilla `GossipContainer.transferFrom`: draws up to ten of the source's gossips,
    /// weighted by their weighted value, and keeps the stronger of each after it decays.
    fn transfer_gossips_from(&self, source: &Self) -> usize {
        let entries = source
            .gossips
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .flat_map(|(target, values)| {
                values
                    .iter()
                    .map(move |(gossip_type, value)| (*target, *gossip_type, *value))
            })
            .collect::<Vec<_>>();
        if entries.is_empty() {
            return 0;
        }
        let ranges = entries
            .iter()
            .scan(0, |end, (_, gossip_type, value)| {
                *end += (value * gossip_type.weight()).abs();
                Some(*end - 1)
            })
            .collect::<Vec<_>>();
        let ranges_end = ranges.last().map_or(0, |last| last + 1);
        if ranges_end <= 0 {
            return 0;
        }
        let random = self.get_random();
        let mut selected = (0..GOSSIP_TRANSFER_COUNT)
            .map(|_| {
                let choice = random.random_range(0..ranges_end);
                ranges.partition_point(|&end| end < choice)
            })
            .collect::<Vec<_>>();
        selected.sort_unstable();
        selected.dedup();

        let mut gossips = self
            .gossips
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for &index in &selected {
            let (target, gossip_type, value) = entries[index];
            let decayed = value - gossip_type.decay_per_transfer();
            if decayed >= MIN_TRANSFERRED_GOSSIP {
                let entry = gossips
                    .entry(target)
                    .or_default()
                    .entry(gossip_type)
                    .or_default();
                // Vanilla `mergeValuesForTransfer`.
                *entry = (*entry).max(decayed);
            }
        }
        selected.len()
    }

    pub(super) fn count_item(&self, item: &Item) -> i32 {
        self.inventory
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter(|stack| stack.item == item)
            .map(|stack| i32::from(stack.item_count))
            .sum()
    }

    /// Vanilla `TradeWithVillager.throwHalfStack`: throws half of the first matching stack
    /// over half full, or what's above 24 of a smaller one.
    fn throw_half_stack(&self, matches: impl Fn(&ItemStack) -> bool, target_pos: Vector3<f64>) {
        let to_throw = {
            let mut inventory = self
                .inventory
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            inventory
                .iter_mut()
                .filter(|stack| !stack.is_empty() && matches(stack))
                .find_map(|stack| {
                    let count = if stack.item_count > stack.get_max_stack_size() / 2 {
                        stack.item_count / 2
                    } else if stack.item_count > THROW_KEEP_COUNT {
                        stack.item_count - THROW_KEEP_COUNT
                    } else {
                        return None;
                    };
                    Some(stack.split(count))
                })
        };
        if let Some(stack) = to_throw.filter(|stack| !stack.is_empty()) {
            throw_item(self.get_entity(), stack, target_pos);
        }
    }
}

/// Vanilla `MoveToTargetSink` for the walk targets the meet behaviors set.
#[derive(Default)]
pub struct MoveToWalkTargetGoal {
    target: Option<WalkTarget>,
    last_target_pos: Option<BlockPos>,
}

impl MoveToWalkTargetGoal {
    fn reached(villager: &VillagerEntity, target: &WalkTarget, world: &World) -> bool {
        target.position(world).is_none_or(|pos| {
            manhattan(
                BlockPos::floored_v(pos),
                villager.get_entity().block_pos.load(),
            ) <= target.close_enough_dist
        })
    }

    fn path_to(&mut self, mob: &dyn Mob, target: &WalkTarget, world: &World) {
        let Some(pos) = target.position(world) else {
            return;
        };
        self.last_target_pos = Some(BlockPos::floored_v(pos));
        let living = &mob.get_mob_entity().living_entity;
        let steered = mob.get_mob_entity().steered();
        let mut navigator = steered.navigator();
        let path = navigator.create_path(living, pos, target.close_enough_dist);
        navigator.move_to_path(path, target.speed, living);
    }
}

impl Goal for MoveToWalkTargetGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(villager) = mob.cast_any().downcast_ref::<VillagerEntity>() else {
            return false;
        };
        let Some(target) = villager.walk_target() else {
            return false;
        };
        let world = mob.get_entity().world.load();
        if Self::reached(villager, &target, &world) {
            villager.set_walk_target(None);
            return false;
        }
        self.target = Some(target);
        true
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        let Some(villager) = mob.cast_any().downcast_ref::<VillagerEntity>() else {
            return false;
        };
        let world = mob.get_entity().world.load();
        self.target.is_some_and(|target| {
            villager.walk_target() == Some(target) && !Self::reached(villager, &target, &world)
        }) && !mob.is_navigator_idle()
    }

    fn start(&mut self, mob: &dyn Mob) {
        if let Some(target) = self.target {
            let world = mob.get_entity().world.load();
            self.path_to(mob, &target, &world);
        }
    }

    fn tick(&mut self, mob: &dyn Mob) {
        // Vanilla re-paths when an entity target moves to another block.
        let Some(target) = self.target else {
            return;
        };
        let world = mob.get_entity().world.load();
        if target
            .position(&world)
            .map(BlockPos::floored_v)
            .is_some_and(|pos| Some(pos) != self.last_target_pos)
        {
            self.path_to(mob, &target, &world);
        }
    }

    fn stop(&mut self, mob: &dyn Mob) {
        if let Some(villager) = mob.cast_any().downcast_ref::<VillagerEntity>()
            && let Some(target) = self.target.take()
        {
            let world = mob.get_entity().world.load();
            if villager.walk_target() == Some(target)
                && (Self::reached(villager, &target, &world) || mob.is_navigator_idle())
            {
                villager.set_walk_target(None);
            }
        }
        mob.get_mob_entity().steered().navigator().stop();
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}
