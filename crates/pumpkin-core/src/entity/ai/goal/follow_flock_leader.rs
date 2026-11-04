use pumpkin_util::random::gameplay::game_random;

use super::{Goal, GoalReach, to_goal_ticks};
use crate::entity::mob::Mob;
use crate::entity::passive::abstract_schooling_fish::LEADER_PATH_SPEED;

const INTERVAL_TICKS: i32 = 200;
const START_TICK_VARIATION: i32 = 20;
const PATH_RECALC_INTERVAL: i32 = 10;

pub struct FollowFlockLeaderGoal {
    time_to_recalc_path: i32,
    next_start_tick: i32,
}

impl Default for FollowFlockLeaderGoal {
    fn default() -> Self {
        Self {
            time_to_recalc_path: 0,
            next_start_tick: Self::next_start_tick(),
        }
    }
}

impl FollowFlockLeaderGoal {
    fn next_start_tick() -> i32 {
        to_goal_ticks(
            INTERVAL_TICKS + game_random().random_range(0..INTERVAL_TICKS) % START_TICK_VARIATION,
        )
    }
}

impl Goal for FollowFlockLeaderGoal {
    fn tick_reach(&self, mob: &dyn Mob, running: bool) -> Option<GoalReach> {
        let fish = mob.as_schooling_fish()?;
        // The leader is a link of every follower. A fish that starts following ticks at once.
        Some(GoalReach {
            // `can_start` looks for a school once its countdown runs out, and makes the
            // neighbours it finds follow.
            typed: (!running && self.next_start_tick <= 0)
                .then(|| fish.school_search())
                .into_iter()
                .collect(),
            speed: LEADER_PATH_SPEED,
            ..GoalReach::default()
        })
    }

    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(fish) = mob.as_schooling_fish() else {
            return false;
        };
        if fish.has_followers() {
            return false;
        }
        if fish.is_follower() {
            return true;
        }
        if self.next_start_tick > 0 {
            self.next_start_tick -= 1;
            return false;
        }
        self.next_start_tick = Self::next_start_tick();
        let neighbors: Vec<_> = fish
            .school_neighbors()
            .into_iter()
            .filter(|other| {
                other
                    .get_mob()
                    .and_then(|mob| mob.as_schooling_fish())
                    .is_some_and(|fish| fish.can_be_followed() || !fish.is_follower())
            })
            .collect();
        let leader = neighbors
            .iter()
            .find(|other| {
                other
                    .get_mob()
                    .and_then(|mob| mob.as_schooling_fish())
                    .is_some_and(crate::entity::passive::abstract_schooling_fish::AbstractSchoolingFish::can_be_followed)
            })
            .cloned()
            .or_else(|| fish.schooling_data().self_reference.upgrade());
        let followers = neighbors
            .into_iter()
            .filter(|other| {
                other
                    .get_mob()
                    .and_then(|mob| mob.as_schooling_fish())
                    .is_some_and(|fish| !fish.is_follower())
            })
            .collect();
        if let Some(leader) = leader
            .as_ref()
            .and_then(|leader| leader.get_mob())
            .and_then(|mob| mob.as_schooling_fish())
        {
            leader.add_followers(followers);
        }
        fish.is_follower()
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        mob.as_schooling_fish()
            .is_some_and(|fish| fish.is_follower() && fish.in_range_of_leader())
    }

    fn start(&mut self, _mob: &dyn Mob) {
        self.time_to_recalc_path = 0;
    }

    fn stop(&mut self, mob: &dyn Mob) {
        if let Some(fish) = mob.as_schooling_fish() {
            fish.stop_following();
        }
    }

    fn tick(&mut self, mob: &dyn Mob) {
        self.time_to_recalc_path -= 1;
        if self.time_to_recalc_path <= 0 {
            self.time_to_recalc_path = self.get_tick_count(PATH_RECALC_INTERVAL);
            if let Some(fish) = mob.as_schooling_fish() {
                fish.path_to_leader();
            }
        }
    }
}
