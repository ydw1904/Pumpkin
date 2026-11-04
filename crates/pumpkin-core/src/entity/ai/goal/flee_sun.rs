use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_util::math::vector3::Vector3;

use super::{Controls, Goal, GoalReach};
use crate::entity::ai::pathfinder::NavigatorGoal;
use crate::entity::mob::Mob;

const HIDE_POS_ATTEMPTS: usize = 10;
const HIDE_POS_HORIZONTAL: i32 = 10;
const HIDE_POS_VERTICAL: i32 = 3;

/// Vanilla `FleeSunGoal`: a burning, bareheaded mob without a target walks into the shade.
pub struct FleeSunGoal {
    speed_modifier: f64,
    wanted: Vector3<f64>,
}

impl FleeSunGoal {
    #[must_use]
    pub const fn new(speed_modifier: f64) -> Self {
        Self {
            speed_modifier,
            wanted: Vector3::new(0.0, 0.0, 0.0),
        }
    }

    /// Vanilla `getHidePos`: a random nearby block out of the sky that the mob likes.
    fn hide_pos(mob: &dyn Mob) -> Option<Vector3<f64>> {
        let entity = mob.get_entity();
        let world = entity.world.load();
        let rng = mob.get_random();
        let pos = entity.block_pos.load();
        for _ in 0..HIDE_POS_ATTEMPTS {
            let candidate = pos.offset(Vector3::new(
                rng.random_range(-HIDE_POS_HORIZONTAL..HIDE_POS_HORIZONTAL),
                rng.random_range(-HIDE_POS_VERTICAL..HIDE_POS_VERTICAL),
                rng.random_range(-HIDE_POS_HORIZONTAL..HIDE_POS_HORIZONTAL),
            ));
            if !world.can_see_sky(&candidate)
                && mob.get_walk_target_value(world.as_ref(), &candidate) < 0.0
            {
                return Some(candidate.to_f64());
            }
        }
        None
    }
}

impl Goal for FleeSunGoal {
    fn tick_reach(&self, _mob: &dyn Mob, _running: bool) -> Option<GoalReach> {
        Some(GoalReach {
            speed: self.speed_modifier,
            block_reads: f64::from(HIDE_POS_HORIZONTAL + 1),
            ..GoalReach::default()
        })
    }

    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let mob_entity = mob.get_mob_entity();
        let entity = &mob_entity.living_entity.entity;
        let world = entity.world.load();
        if mob_entity.get_target().is_some()
            || !world.is_bright_outside()
            || !entity.is_on_fire()
            || !world.can_see_sky(&entity.block_pos.load())
            || !mob_entity
                .living_entity
                .entity_equipment
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(&EquipmentSlot::HEAD)
                .is_empty()
        {
            return false;
        }
        let Some(wanted) = Self::hide_pos(mob) else {
            return false;
        };
        self.wanted = wanted;
        true
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        !mob.is_navigator_idle()
    }

    fn start(&mut self, mob: &dyn Mob) {
        let pos = mob.get_entity().pos.load();
        mob.get_mob_entity()
            .steered()
            .navigator()
            .set_progress(NavigatorGoal::new(pos, self.wanted, self.speed_modifier));
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}
