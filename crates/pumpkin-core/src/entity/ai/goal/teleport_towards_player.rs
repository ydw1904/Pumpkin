use std::sync::Arc;

use super::track_target::TrackTargetGoal;
use super::{Controls, Goal, GoalReach, to_goal_ticks};
use crate::entity::EntityBase;
use crate::entity::ai::target_predicate::TargetPredicate;
use crate::entity::mob::Mob;
use crate::entity::mob::enderman::{EndermanEntity, PLAYER_EYE_HEIGHT};
use crate::entity::mob::neutral::NeutralMob;
use crate::entity::player::Player;
use crate::plugin::api::events::entity::entity_target::TargetReason;
use crate::world::World;
use pumpkin_data::attributes::Attributes;

const STARE_CLOSE_DISTANCE_SQ: f64 = 16.0;
const TELEPORT_FAR_DISTANCE_SQ: f64 = 256.0;
/// Ticks a far target goes unseen before the enderman teleports towards it.
const TELEPORT_DELAY: i32 = 30;

pub struct TeleportTowardsPlayerGoal {
    enderman: Arc<EndermanEntity>,
    track_target_goal: TrackTargetGoal,
    target_player: Option<Arc<dyn EntityBase>>,
    committed_target: Option<Arc<dyn EntityBase>>,
    target_predicate: TargetPredicate,
    warmup: i32,
    unseen_ticks: i32,
}

impl TeleportTowardsPlayerGoal {
    pub fn new(enderman: Arc<EndermanEntity>) -> Self {
        let track_target_goal = TrackTargetGoal::with_default(false);
        let mut target_predicate = TargetPredicate::create_attackable();
        target_predicate.base_max_distance = enderman
            .mob_entity
            .living_entity
            .get_attribute_value(&Attributes::FOLLOW_RANGE);
        Self {
            enderman,
            track_target_goal,
            target_player: None,
            committed_target: None,
            target_predicate,
            warmup: 0,
            unseen_ticks: 0,
        }
    }

    /// Vanilla `isAngerInducing`: stared at, or held against by the grudge.
    fn is_anger_inducing(&self, player: &Player, world: &World) -> bool {
        self.enderman.is_player_staring(player) || self.enderman.is_angry_at(player, world)
    }

    fn find_staring_player(&self) -> Option<Arc<Player>> {
        let entity = &self.enderman.mob_entity.living_entity.entity;
        let world = entity.world.load();
        let pos = entity.pos.load();

        // A nearer player that is not anger-inducing must not hide a farther one that is.
        // Players beyond the conditions' range can't pass them.
        let enderman: &dyn EntityBase = self.enderman.as_ref();
        let range = self.target_predicate.max_distance();
        world.get_nearest_player(pos, range, |player| {
            self.target_predicate
                .test(&world, Some(enderman), player.as_ref())
                && self.is_anger_inducing(player, &world)
        })
    }
}

impl Goal for TeleportTowardsPlayerGoal {
    fn tick_reach(&self, mob: &dyn Mob, running: bool) -> Option<GoalReach> {
        let ids: Vec<i32> = self
            .target_player
            .iter()
            .chain(&self.committed_target)
            .map(|target| target.get_entity().entity_id)
            .collect();
        let mut reach = GoalReach {
            // The stare check traces from the player's eyes.
            sight: ids.clone(),
            links: ids,
            ..GoalReach::default()
        };
        let world = mob.get_entity().world.load();
        if !running {
            // The search checks the stare of every player in range.
            reach.players = world
                .player_search
                .attackable_reach(self.target_predicate.max_distance());
            reach.block_reads = reach.players;
            return Some(reach);
        }
        // Only a committed target, or the mob's own one, gets the teleports of `tick`, which
        // never runs on the tick that starts the goal.
        if self.target_player.is_none()
            && let Some(target) = self
                .committed_target
                .clone()
                .or_else(|| mob.get_mob_entity().get_target())
        {
            let dist_sq = mob
                .get_entity()
                .pos
                .load()
                .squared_distance_to_vec(&target.get_entity().pos.load());
            if dist_sq < STARE_CLOSE_DISTANCE_SQ
                || (dist_sq > TELEPORT_FAR_DISTANCE_SQ
                    && self.unseen_ticks >= to_goal_ticks(TELEPORT_DELAY))
            {
                reach.teleport = Some(self.enderman.teleport_area(1));
            }
        }
        Some(reach)
    }

    fn can_start(&mut self, _mob: &dyn Mob) -> bool {
        let Some(player) = self.find_staring_player() else {
            return false;
        };
        self.target_player = Some(player);
        true
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        if let Some(target) = &self.target_player
            && let Some(player) = target.get_player()
        {
            let world = self.enderman.mob_entity.living_entity.entity.world.load();
            if !player.living_entity.is_alive() || !self.is_anger_inducing(player, &world) {
                return false;
            }
            let player_pos = player.get_entity().pos.load();
            let mut look_control = mob
                .get_mob_entity()
                .look_control
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            look_control.look_at_with_range(
                player_pos.x,
                player_pos.y + PLAYER_EYE_HEIGHT,
                player_pos.z,
                10.0,
                10.0,
            );
            true
        } else if self.target_player.is_some() {
            false
        } else if let Some(target) = &self.committed_target {
            if target
                .get_living_entity()
                .is_none_or(|living| !living.is_alive())
            {
                return false;
            }
            let mob_entity = mob.get_mob_entity();
            let dist_sq = mob_entity
                .living_entity
                .entity
                .pos
                .load()
                .squared_distance_to_vec(&target.get_entity().pos.load());
            let follow_range = mob_entity
                .living_entity
                .get_attribute_value(&Attributes::FOLLOW_RANGE);
            if dist_sq > follow_range * follow_range {
                return false;
            }
            let needs_reset = mob_entity.get_target().is_none();
            if needs_reset {
                mob.set_mob_target(Some(target.clone()), TargetReason::ClosestPlayer);
            }
            true
        } else {
            self.track_target_goal.should_continue(mob)
        }
    }

    fn start(&mut self, _mob: &dyn Mob) {
        self.warmup = to_goal_ticks(5);
        self.unseen_ticks = 0;
        self.enderman.set_provoked(true);
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let external_target = mob.get_mob_entity().get_target().clone();
        if external_target.is_none()
            && self.target_player.is_none()
            && self.committed_target.is_none()
        {
            return;
        }

        if self.target_player.is_some() {
            self.warmup -= 1;
            if self.warmup <= 0 {
                let target = self.target_player.take();
                self.committed_target.clone_from(&target);
                self.enderman.set_target(target);
                self.track_target_goal.start(mob);
            }
            return;
        }

        let target = self.committed_target.clone().or(external_target);
        let Some(target) = target else {
            return;
        };
        if mob.get_entity().has_vehicle() {
            return;
        }

        let entity = &mob.get_mob_entity().living_entity.entity;
        let pos = entity.pos.load();
        let target_pos = target.get_entity().pos.load();
        let dist_sq = pos.squared_distance_to_vec(&target_pos);

        if let Some(player) = target.get_player()
            && self.enderman.is_player_staring(player)
        {
            if dist_sq < STARE_CLOSE_DISTANCE_SQ {
                self.enderman.teleport_randomly();
            }
            self.unseen_ticks = 0;
        } else if dist_sq > TELEPORT_FAR_DISTANCE_SQ {
            // Vanilla `teleportTime++ >= adjustedTickDelay(30)`: a failed teleport retries on
            // the next tick.
            let waited = self.unseen_ticks;
            self.unseen_ticks += 1;
            if waited >= to_goal_ticks(TELEPORT_DELAY)
                && self.enderman.teleport_towards(target.as_ref())
            {
                self.unseen_ticks = 0;
            }
        }
    }

    fn stop(&mut self, _mob: &dyn Mob) {
        self.target_player = None;
        self.committed_target = None;
        self.enderman.set_target(None);
    }

    fn controls(&self) -> Controls {
        Controls::TARGET
    }
}
