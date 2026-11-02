use pumpkin_util::math::position::BlockPos;

use super::{Controls, Goal};
use crate::entity::{ai::pathfinder::NavigatorGoal, mob::Mob};

pub struct WorkAtJobSiteGoal {
    speed: f64,
    target: Option<BlockPos>,
}

impl WorkAtJobSiteGoal {
    #[must_use]
    pub const fn new(speed: f64) -> Self {
        Self {
            speed,
            target: None,
        }
    }

    /// The potential job site `GoToPotentialJobSite` walks to, or the job site during work
    /// hours.
    fn walk_target(mob: &dyn Mob) -> Option<BlockPos> {
        if let Some(potential) = mob.get_potential_job_site_walk_target() {
            return Some(potential);
        }
        let world = mob.get_mob_entity().living_entity.entity.world.load();
        let daytime = world
            .level_time
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .query_daytime();
        mob.get_job_site()
            .filter(|_| (2_000..9_000).contains(&daytime))
    }
}

impl Goal for WorkAtJobSiteGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(target) = Self::walk_target(mob) else {
            return false;
        };
        let position = mob.get_mob_entity().living_entity.entity.pos.load();
        if target.to_centered_f64().squared_distance_to_vec(&position) < 1.73f64.powi(2) {
            return false;
        }
        self.target = Some(target);
        true
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        let Some(target) = self.target else {
            return false;
        };
        if Self::walk_target(mob) != Some(target) {
            return false;
        }
        let entity = &mob.get_mob_entity().living_entity.entity;
        target
            .to_centered_f64()
            .squared_distance_to_vec(&entity.pos.load())
            >= 1.73f64.powi(2)
            && !mob.is_navigator_idle()
    }

    fn start(&mut self, mob: &dyn Mob) {
        if let Some(target) = self.target {
            let entity = &mob.get_mob_entity().living_entity.entity;
            mob.get_mob_entity()
                .steered()
                .navigator()
                .set_progress(NavigatorGoal::new(
                    entity.pos.load(),
                    target.to_centered_f64(),
                    self.speed,
                ));
        }
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.target = None;
        mob.get_mob_entity().steered().navigator().stop();
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}
