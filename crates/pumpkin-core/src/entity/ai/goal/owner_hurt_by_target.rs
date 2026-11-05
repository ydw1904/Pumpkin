use super::track_target::TrackTargetGoal;
use super::{Controls, Goal, GoalReach};
use crate::entity::EntityBase;
use crate::entity::ai::target_predicate::TargetPredicate;
use crate::entity::mob::Mob;
use crate::plugin::api::events::entity::entity_target::TargetReason;
use std::sync::Arc;

pub struct OwnerHurtByTargetGoal {
    track_target_goal: TrackTargetGoal,
    target: Option<Arc<dyn EntityBase>>,
    target_predicate: TargetPredicate,
    timestamp: i32,
}

impl OwnerHurtByTargetGoal {
    #[must_use]
    pub fn new() -> Box<Self> {
        Box::new(Self {
            track_target_goal: TrackTargetGoal::with_default(false),
            target: None,
            target_predicate: TargetPredicate::create_attackable(),
            timestamp: 0,
        })
    }
}

impl Goal for OwnerHurtByTargetGoal {
    // The owner's attacker or victim can change in any entity's turn, and starting checks the
    // sight to it, which no bound covers. A pet that can't start only keeps its target.
    fn tick_reach(&self, mob: &dyn Mob, _running: bool) -> Option<GoalReach> {
        if mob.is_tamed()
            && !mob.ordered_to_sit()
            && mob.get_owner_uuid().is_some_and(|uuid| {
                mob.get_entity()
                    .world
                    .load()
                    .get_player_by_uuid(uuid)
                    .is_some()
            })
        {
            return None;
        }
        let target: Vec<i32> = self.track_target_goal.target_mob_id().into_iter().collect();
        Some(GoalReach {
            links: target.clone(),
            sight: target,
            ..GoalReach::default()
        })
    }

    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        if !mob.is_tamed() || mob.ordered_to_sit() {
            return false;
        }

        let Some(owner_uuid) = mob.get_owner_uuid() else {
            return false;
        };

        let world = mob.get_mob_entity().living_entity.entity.world.load_full();
        let Some(owner) = world.get_player_by_uuid(owner_uuid) else {
            return false;
        };

        let other = owner.living_entity.get_last_hurt_by_mob();
        let timestamp = owner.living_entity.get_last_hurt_by_mob_timestamp();
        if timestamp == self.timestamp {
            return false;
        }

        let Some(other) = other else {
            return false;
        };

        if !self
            .track_target_goal
            .can_track(mob, Some(other.as_ref()), &self.target_predicate)
            || !mob.can_attack_with_owner(other.as_ref(), &*owner)
        {
            return false;
        }

        self.target = Some(other);
        true
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.track_target_goal.should_continue(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.track_target_goal.start_targeting(
            mob,
            self.target.clone(),
            TargetReason::TargetAttackedOwner,
        );

        if let Some(owner_uuid) = mob.get_owner_uuid() {
            let world = mob.get_mob_entity().living_entity.entity.world.load_full();
            if let Some(owner) = world.get_player_by_uuid(owner_uuid) {
                self.timestamp = owner.living_entity.get_last_hurt_by_mob_timestamp();
            }
        }

        self.track_target_goal.start(mob);
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.target = None;
        self.track_target_goal.stop(mob);
    }

    fn controls(&self) -> Controls {
        self.track_target_goal.controls()
    }
}
