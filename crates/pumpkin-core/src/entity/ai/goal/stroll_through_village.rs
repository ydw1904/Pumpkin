use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::chunk::ChunkHeightmapType;
use pumpkin_world::poi::SectionPos;

use super::{Controls, Goal, to_goal_ticks};
use crate::entity::ai::pathfinder::NavigatorGoal;
use crate::entity::ai::util::land_random_pos;
use crate::entity::mob::Mob;

/// Vanilla `DISTANCE_THRESHOLD`: closer than this the mob is left to its current path.
const DISTANCE_THRESHOLD: f64 = 10.0;

/// Vanilla `StrollThroughVillageGoal`: at night, near a village, walks towards a spot deeper in it.
pub struct StrollThroughVillageGoal {
    interval: i32,
    wanted_pos: Option<BlockPos>,
}

impl StrollThroughVillageGoal {
    #[must_use]
    pub const fn new(interval: i32) -> Self {
        Self {
            interval: to_goal_ticks(interval),
            wanted_pos: None,
        }
    }
}

impl Goal for StrollThroughVillageGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let entity = mob.get_entity();
        let world = entity.world.load_full();
        if mob.get_controlling_passenger().is_some()
            || world.is_bright_outside()
            || mob.get_random().random_range(0..self.interval) != 0
        {
            return false;
        }
        if !world.is_close_to_village(&entity.block_pos.load(), 6) {
            return false;
        }
        self.wanted_pos = land_random_pos::get_pos_weighted(mob, 15, 7, |pos| {
            -f64::from(world.sections_to_village(SectionPos::of(pos)))
        })
        .map(BlockPos::floored_v);
        self.wanted_pos.is_some()
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        let Some(wanted) = self.wanted_pos else {
            return false;
        };
        !mob.is_navigator_idle()
            && mob.get_mob_entity().steered().navigator().get_target_pos() == Some(wanted)
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let Some(wanted) = self.wanted_pos else {
            return;
        };
        let entity = mob.get_entity();
        let pos = entity.pos.load();
        if !mob.is_navigator_idle()
            || wanted.to_centered_f64().squared_distance_to_vec(&pos)
                < DISTANCE_THRESHOLD * DISTANCE_THRESHOLD
        {
            return;
        }
        // A point ten blocks along the way to 60% of the distance to the wanted spot.
        let wanted_bottom = wanted.to_f64();
        let long_distance_target = (pos - wanted_bottom) * 0.4 + wanted_bottom;
        let move_target = (long_distance_target - pos).normalize() * DISTANCE_THRESHOLD + pos;
        let path_target = BlockPos::floored_v(move_target);
        let world = entity.world.load();
        let surface_y = world.get_heightmap_height(
            ChunkHeightmapType::MotionBlockingNoLeaves,
            path_target.0.x,
            path_target.0.z,
        ) + 1;
        // Pumpkin's navigation resolves the path later, so vanilla's `moveRandomly` fallback for
        // an unreachable target has nothing to react to here.
        mob.get_mob_entity()
            .steered()
            .navigator()
            .set_progress(NavigatorGoal::new(
                pos,
                Vector3::new(
                    f64::from(path_target.0.x),
                    f64::from(surface_y),
                    f64::from(path_target.0.z),
                ),
                1.0,
            ));
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}
