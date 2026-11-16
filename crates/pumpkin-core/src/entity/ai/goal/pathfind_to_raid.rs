use pumpkin_util::math::vector3::Vector3;

use crate::entity::ai::goal::{Controls, Goal};
use crate::entity::ai::util::default_random_pos;
use crate::entity::mob::Mob;
use crate::entity::mob::raider::is_raider;
use crate::world::raid::can_join_raid;

/// Vanilla `PathfindToRaidGoal`: a raider outside the village walks towards its raid's centre
/// and brings the raiders it passes into the raid.
#[derive(Default)]
pub struct PathfindToRaidGoal {
    recruitment_tick: i32,
}

impl PathfindToRaidGoal {
    const RECRUITMENT_SEARCH_TICK_DELAY: i32 = 20;
    const RECRUITMENT_RANGE: f64 = 16.0;
    const SPEED_MODIFIER: f64 = 1.0;

    /// The raid's centre while the raider is in an unfinished raid and outside a village.
    fn raid_center_calling(mob: &dyn Mob) -> Option<(i32, Vector3<f64>)> {
        let raid_id = mob.as_raider()?.get_raider_data().raid_id.load()?;
        let world = mob.get_entity().world.load();
        let center = {
            let raids = world
                .raids
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let raid = raids.get(raid_id).filter(|raid| raid.is_active())?;
            if raid.is_over() {
                return None;
            }
            raid.center
        };
        (!world.is_village(&mob.get_entity().block_pos.load())).then(|| {
            (
                raid_id,
                Vector3::new(
                    f64::from(center.0.x) + 0.5,
                    f64::from(center.0.y),
                    f64::from(center.0.z) + 0.5,
                ),
            )
        })
    }

    /// Vanilla `recruitNearby`: raiders within 16 blocks that may join raids join this one.
    fn recruit_nearby(mob: &dyn Mob, raid_id: i32) {
        let entity = mob.get_entity();
        let world = entity.world.load_full();
        let area = entity.bounding_box.load().expand(
            Self::RECRUITMENT_RANGE,
            Self::RECRUITMENT_RANGE,
            Self::RECRUITMENT_RANGE,
        );
        let candidates = world.get_entities_of_types_at_box(&area, is_raider);
        let mut raids = world
            .raids
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(raid) = raids.get_mut(raid_id).filter(|raid| raid.is_active()) else {
            return;
        };
        for candidate in candidates {
            if let Some(raider) = candidate.get_mob().and_then(|mob| mob.as_raider())
                && !raider.has_active_raid()
                && can_join_raid(raider)
            {
                raid.recruit(&world, raid.get_groups_spawned(), raider);
            }
        }
    }
}

impl Goal for PathfindToRaidGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        mob.get_mob_entity().get_target().is_none()
            && mob.get_controlling_passenger().is_none()
            && Self::raid_center_calling(mob).is_some()
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        Self::raid_center_calling(mob).is_some()
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let Some((raid_id, center)) = Self::raid_center_calling(mob) else {
            return;
        };
        let tick_count = mob
            .get_entity()
            .tick_count
            .load(std::sync::atomic::Ordering::Relaxed);
        if tick_count > self.recruitment_tick {
            self.recruitment_tick = tick_count + Self::RECRUITMENT_SEARCH_TICK_DELAY;
            Self::recruit_nearby(mob, raid_id);
        }

        let steered = mob.get_mob_entity().steered();
        let path_finding = !steered.navigator().is_done();
        if !path_finding
            && let Some(dest) =
                default_random_pos::get_pos_towards(mob, 15, 4, center, std::f64::consts::FRAC_PI_2)
        {
            steered.navigator().move_to_coords(
                dest.x,
                dest.y,
                dest.z,
                Self::SPEED_MODIFIER,
                &steered.mob_entity().living_entity,
            );
        }
    }
}
