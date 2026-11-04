use super::{Controls, Goal, GoalReach, to_goal_ticks};
use crate::entity::EntityBase;
use crate::entity::mob::Mob;
use pumpkin_util::math::vector3::Vector3;
use std::sync::Arc;
use std::sync::atomic::Ordering;

pub struct LeapAtTargetGoal {
    goal_control: Controls,
    yd: f32,
    target: Option<Arc<dyn EntityBase>>,
}

impl LeapAtTargetGoal {
    #[must_use]
    pub const fn new(yd: f32) -> Self {
        Self {
            goal_control: Controls::MOVE.union(Controls::JUMP),
            yd,
            target: None,
        }
    }
}

/// Vanilla `LeapAtTargetGoal.start`: the horizontal leap speed, before a fifth of the old velocity.
const LEAP_SPEED: f64 = 0.4;

impl Goal for LeapAtTargetGoal {
    fn tick_reach(&self, _mob: &dyn Mob, _running: bool) -> Option<GoalReach> {
        // Starting reads the mob's own target and sets its velocity, which keeps a fifth of the
        // old one.
        Some(GoalReach {
            impulse: LEAP_SPEED,
            links: self
                .target
                .iter()
                .map(|target| target.get_entity().entity_id)
                .collect(),
            ..GoalReach::default()
        })
    }

    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        if mob.get_controlling_passenger().is_some() {
            return false;
        }

        let mob_entity = mob.get_mob_entity();
        let entity = &mob_entity.living_entity.entity;

        if !entity.on_ground.load(Ordering::Relaxed) {
            return false;
        }

        let target = mob_entity.get_target();
        let Some(target) = target else {
            return false;
        };

        if !target.get_entity().is_alive() {
            return false;
        }

        let mob_pos = entity.pos.load();
        let target_pos = target.get_entity().pos.load();
        let dist_sq = mob_pos.squared_distance_to_vec(&target_pos);

        if !(4.0..=16.0).contains(&dist_sq) {
            return false;
        }

        if mob.get_random().random_range(0..to_goal_ticks(5).max(1)) != 0 {
            return false;
        }

        self.target = Some(target);
        true
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        let entity = &mob.get_mob_entity().living_entity.entity;
        !entity.on_ground.load(Ordering::Relaxed)
    }

    fn start(&mut self, mob: &dyn Mob) {
        let Some(target) = &self.target else {
            return;
        };

        let mob_entity = mob.get_mob_entity();
        let entity = &mob_entity.living_entity.entity;
        let mob_pos = entity.pos.load();
        let target_pos = target.get_entity().pos.load();
        let movement = entity.velocity.load();

        let dx = target_pos.x - mob_pos.x;
        let dz = target_pos.z - mob_pos.z;
        let dist_xz_sq = dx * dx + dz * dz;

        let delta = if dist_xz_sq > 1.0E-7 {
            let dist_xz = dist_xz_sq.sqrt();
            Vector3::new(
                (dx / dist_xz) * LEAP_SPEED + movement.x * 0.2,
                f64::from(self.yd),
                (dz / dist_xz) * LEAP_SPEED + movement.z * 0.2,
            )
        } else {
            Vector3::new(dx, f64::from(self.yd), dz)
        };

        entity.set_velocity(delta);
    }

    fn stop(&mut self, _mob: &dyn Mob) {
        self.target = None;
    }

    fn controls(&self) -> Controls {
        self.goal_control
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::Entity;
    use crate::entity::ai::test_support::world;
    use crate::entity::mob::spider::SpiderEntity;
    use crate::entity::passive::cow::CowEntity;
    use pumpkin_data::entity::EntityType;

    #[tokio::test]
    async fn a_leap_stays_within_the_audited_motion() -> Result<(), Box<dyn std::error::Error>> {
        let folder = tempfile::tempdir()?;
        let world = world(&folder);
        let spider = SpiderEntity::new(Entity::new(
            world.clone(),
            Vector3::new(0.5, 64.0, 0.5),
            &EntityType::SPIDER,
        ));
        let cow = CowEntity::new(Entity::new(
            world.clone(),
            Vector3::new(3.5, 64.0, 0.5),
            &EntityType::COW,
        ));
        let mob = spider.get_mob_entity();
        mob.set_target(Some(cow));
        mob.living_entity
            .entity
            .on_ground
            .store(true, Ordering::Relaxed);
        let reach = mob
            .audited_reach(spider.as_ref())
            .ok_or("missing spider reach")?;
        let mut goal = LeapAtTargetGoal::new(0.4);
        // The start roll passes one time in three.
        assert!((0..200).any(|_| goal.can_start(spider.as_ref())));
        goal.start(spider.as_ref());
        let velocity = mob.living_entity.entity.velocity.load();
        world.level.shutdown().await;
        assert!(velocity.x.hypot(velocity.z) <= reach.motion.0);
        assert!(velocity.y.abs() <= reach.motion.1);
        Ok(())
    }
}
