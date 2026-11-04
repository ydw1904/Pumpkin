use pumpkin_data::data_component_impl::EquipmentSlot;

use super::{Controls, Goal, GoalReach};
use crate::entity::mob::Mob;

/// Vanilla `RestrictSunGoal`: while the sun is up, a bareheaded mob's paths stop short of the
/// open sky.
#[derive(Default)]
pub struct RestrictSunGoal;

impl RestrictSunGoal {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Goal for RestrictSunGoal {
    // Path trimming only reads the nodes of the mob's route.
    fn tick_reach(&self, _mob: &dyn Mob, _running: bool) -> Option<GoalReach> {
        Some(GoalReach::default())
    }

    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let mob_entity = mob.get_mob_entity();
        mob.get_entity().world.load().is_bright_outside()
            && mob_entity
                .living_entity
                .entity_equipment
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(&EquipmentSlot::HEAD)
                .is_empty()
            && mob_entity.steered().navigator().can_navigate_ground()
    }

    fn start(&mut self, mob: &dyn Mob) {
        mob.get_mob_entity()
            .steered()
            .navigator()
            .set_avoid_sun(true);
    }

    fn stop(&mut self, mob: &dyn Mob) {
        mob.get_mob_entity()
            .steered()
            .navigator()
            .set_avoid_sun(false);
    }

    fn controls(&self) -> Controls {
        Controls::empty()
    }
}
