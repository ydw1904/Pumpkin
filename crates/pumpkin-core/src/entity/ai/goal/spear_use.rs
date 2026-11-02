use pumpkin_data::data_component_impl::{EquipmentSlot, KineticWeaponImpl};
use pumpkin_data::item_stack::ItemStack;
use pumpkin_util::Hand;
use pumpkin_util::math::vector3::Vector3;

use super::{Controls, Goal, to_goal_ticks};
use crate::entity::ai::pathfinder::path::Path;
use crate::entity::ai::util::land_random_pos;
use crate::entity::mob::Mob;
use crate::item::items::spear::SpearItem;

const MIN_REPOSITION_DISTANCE: f64 = 6.0;
const MAX_REPOSITION_DISTANCE: f64 = 7.0;
const MIN_COOLDOWN_DISTANCE: f64 = 9.0;
const MAX_COOLDOWN_DISTANCE: f64 = 11.0;
const MAX_FLEEING_TIME: i32 = to_goal_ticks(100);
/// Vanilla `getPosAway`'s vertical range in `SpearUseGoal`.
const AWAY_VERTICAL_RANGE: i32 = 7;

/// Vanilla `SpearUseGoal`: a mob holding a kinetic weapon closes in, lowers the spear and
/// charges through its target, then backs off before the next charge.
pub struct SpearUseGoal {
    state: Option<SpearUseState>,
    speed_modifier_when_charging: f64,
    speed_modifier_when_repositioning: f64,
    approach_distance_sq: f64,
    target_in_range_radius_sq: f64,
}

/// Vanilla `SpearUseGoal.SpearUseState`.
struct SpearUseState {
    engage_time: i32,
    fleeing_time: i32,
    away_pos: Option<Vector3<f64>>,
    done: bool,
    last_path: Option<Path>,
}

impl SpearUseState {
    const fn new() -> Self {
        Self {
            engage_time: -1,
            fleeing_time: -1,
            away_pos: None,
            done: false,
            last_path: None,
        }
    }

    const fn not_engaged_yet(&self) -> bool {
        self.engage_time < 0
    }

    const fn tick_and_check_engagement(&mut self) -> bool {
        if self.engage_time > 0 {
            self.engage_time -= 1;
            if self.engage_time == 0 {
                return true;
            }
        }
        false
    }

    const fn tick_and_check_fleeing(&mut self) -> bool {
        if self.fleeing_time > 0 {
            self.fleeing_time += 1;
            if self.fleeing_time > MAX_FLEEING_TIME {
                self.done = true;
                return true;
            }
        }
        false
    }
}

impl SpearUseGoal {
    #[must_use]
    pub fn new(
        speed_modifier_when_charging: f64,
        speed_modifier_when_repositioning: f64,
        approach_distance: f32,
        target_in_range_radius: f32,
    ) -> Box<Self> {
        Box::new(Self {
            state: None,
            speed_modifier_when_charging,
            speed_modifier_when_repositioning,
            approach_distance_sq: f64::from(approach_distance * approach_distance),
            target_in_range_radius_sq: f64::from(target_in_range_radius * target_in_range_radius),
        })
    }

    fn main_hand_item(mob: &dyn Mob) -> ItemStack {
        mob.get_mob_entity()
            .living_entity
            .entity_equipment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&EquipmentSlot::MAIN_HAND)
    }

    fn able_to_attack(mob: &dyn Mob) -> bool {
        mob.get_mob_entity().get_target().is_some()
            && Self::main_hand_item(mob)
                .get_data_component::<KineticWeaponImpl>()
                .is_some()
    }

    fn is_using_item(mob: &dyn Mob) -> bool {
        mob.get_mob_entity()
            .living_entity
            .item_in_use
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some()
    }

    fn kinetic_weapon_use_duration(mob: &dyn Mob) -> i32 {
        let duration = Self::main_hand_item(mob)
            .get_data_component::<KineticWeaponImpl>()
            .map_or(0, |weapon| {
                weapon.delay_ticks
                    + weapon
                        .damage_conditions
                        .as_ref()
                        .map_or(0, |condition| condition.max_duration_ticks)
            });
        to_goal_ticks(duration)
    }

    /// Vanilla `PathNavigation.moveTo` towards `destination`, every tick. Vanilla's navigation
    /// keeps a finished path, and a `moveTo` along the same route leaves it finished, which is
    /// how this goal notices it arrived. Pumpkin's navigation drops finished paths, so the
    /// state remembers the last route instead.
    fn move_to(
        mob: &dyn Mob,
        last_path: &mut Option<Path>,
        destination: Vector3<f64>,
        reach_range: i32,
        speed: f64,
    ) {
        let steered = mob.get_mob_entity().steered();
        let living = &steered.mob_entity().living_entity;
        let mut navigator = steered.navigator();
        let path = navigator.create_path(living, destination, reach_range);
        if navigator.is_done()
            && path
                .as_ref()
                .zip(last_path.as_ref())
                .is_some_and(|(path, last_path)| path.same_as_path(last_path))
        {
            return;
        }
        last_path.clone_from(&path);
        navigator.move_to_path(path, speed, living);
    }

    fn is_navigation_done(mob: &dyn Mob) -> bool {
        mob.get_mob_entity().steered().navigator().is_done()
    }
}

impl Goal for SpearUseGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        Self::able_to_attack(mob) && !Self::is_using_item(mob)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.state.as_ref().is_some_and(|state| !state.done) && Self::able_to_attack(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        mob.get_mob_entity().set_attacking(true);
        self.state = Some(SpearUseState::new());
    }

    fn stop(&mut self, mob: &dyn Mob) {
        let mob_entity = mob.get_mob_entity();
        mob_entity.steered().navigator().stop();
        mob_entity.set_attacking(false);
        self.state = None;
        mob_entity.living_entity.clear_active_hand();
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let Some(state) = self.state.as_mut() else {
            return;
        };
        let mob_entity = mob.get_mob_entity();
        let Some(target) = mob_entity.get_target() else {
            return;
        };
        let entity = mob.get_entity();
        let target_pos = target.get_entity().pos.load();
        let target_dist_sq = entity.pos.load().squared_distance_to_vec(&target_pos);
        let speed_modifier = entity
            .get_root_vehicle()
            .and_then(|mount| mount.get_mob().map(Mob::charge_speed_modifier))
            .unwrap_or(1.0);
        let speed_modifier = f64::from(speed_modifier);
        let mount_distance = if entity.has_vehicle() { 2.0 } else { 0.0 };

        mob_entity.look_at(target.as_ref(), 30.0, 30.0);
        mob_entity
            .look_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .look_at_entity_with_range(&target, 30.0, 30.0);

        if state.not_engaged_yet() {
            if target_dist_sq > self.approach_distance_sq {
                Self::move_to(
                    mob,
                    &mut state.last_path,
                    target_pos,
                    0,
                    speed_modifier * self.speed_modifier_when_repositioning,
                );
                return;
            }
            state.engage_time = Self::kinetic_weapon_use_duration(mob);
            // Vanilla `startUsingItem(MAIN_HAND)`, which ignores an empty hand.
            let stack = Self::main_hand_item(mob);
            if !stack.is_empty() && !Self::is_using_item(mob) {
                mob_entity.living_entity.set_active_hand(
                    Hand::Right,
                    stack,
                    SpearItem::USE_DURATION,
                );
            }
        }

        if state.tick_and_check_engagement() {
            mob_entity.living_entity.clear_active_hand();
            let distance = target_dist_sq.sqrt();
            state.away_pos = land_random_pos::get_pos_away(
                mob,
                (MIN_COOLDOWN_DISTANCE + mount_distance - distance).max(0.0),
                (MAX_COOLDOWN_DISTANCE + mount_distance - distance).max(1.0),
                AWAY_VERTICAL_RANGE,
                target_pos,
            );
            state.fleeing_time = 1;
        }

        if state.tick_and_check_fleeing() {
            return;
        }
        if let Some(away_pos) = state.away_pos {
            Self::move_to(
                mob,
                &mut state.last_path,
                away_pos,
                1,
                speed_modifier * self.speed_modifier_when_repositioning,
            );
            if Self::is_navigation_done(mob) {
                if state.fleeing_time > 0 {
                    state.done = true;
                    return;
                }
                state.away_pos = None;
            }
        } else {
            Self::move_to(
                mob,
                &mut state.last_path,
                target_pos,
                0,
                speed_modifier * self.speed_modifier_when_charging,
            );
            if target_dist_sq < self.target_in_range_radius_sq || Self::is_navigation_done(mob) {
                let distance = target_dist_sq.sqrt();
                state.away_pos = land_random_pos::get_pos_away(
                    mob,
                    MIN_REPOSITION_DISTANCE + mount_distance - distance,
                    MAX_REPOSITION_DISTANCE + mount_distance - distance,
                    AWAY_VERTICAL_RANGE,
                    target_pos,
                );
            }
        }
    }

    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK
    }
}
