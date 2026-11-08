use super::{Controls, Goal, GoalReach, to_goal_ticks};

use crate::entity::ageable::AgeableMob;
use crate::entity::ai::behavior::hunting::TargetPredicateFn;
use crate::entity::ai::goal::revenge::MobFilter;
use crate::entity::ai::goal::track_target::TrackTargetGoal;
use crate::entity::ai::target_match::{SearchArea, TargetMatch};
use crate::entity::ai::target_predicate::TargetPredicate;
use crate::entity::living::LivingEntity;
use crate::entity::mob::Mob;
use crate::entity::mob::neutral::{NeutralMob, find_by_uuid};
use crate::entity::{EntityBase, mob::MobEntity};
use crate::plugin::api::events::entity::entity_target::TargetReason;
use crate::world::World;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::entity::EntityType;
use pumpkin_util::math::boundingbox::BoundingBox;
use std::sync::Arc;
use uuid::Uuid;

const DEFAULT_RECIPROCAL_CHANCE: i32 = 10;

/// Extra gate on top of the target predicate, for mobs that pick targets conditionally.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub enum TargetCondition {
    #[default]
    Always,
    /// Only what the mob holds a grudge against. Neutral mobs.
    AngryAt,
    /// Nothing at all in daylight. Spiders.
    NoDaylight,
    /// Nothing at all while the mob is a baby. (Polar bears hunting foxes)
    Adult,
    /// Only starts while the mob is untamed, and re-runs the search test to continue. (Wolves hunting prey)
    Untamed,
    /// Only starts while the mob is on a team. (Shulkers defending their team)
    OnTeam,
}

impl TargetCondition {
    /// Checked once per search attempt.
    fn allows_search(self, mob: &dyn Mob) -> bool {
        match self {
            // A calm mob matches nobody: skip the search. Grudge check first, it is lock-free.
            Self::AngryAt => mob.as_neutral().is_some_and(|neutral| {
                neutral.get_persistent_anger_target().is_some() || neutral.is_angry()
            }),
            // TODO: `Mob::is_baby()` once baby state is unified.
            Self::Adult => !mob.as_ageable().is_some_and(AgeableMob::is_baby),
            Self::Untamed => !mob.is_tamed(),
            Self::Always | Self::NoDaylight | Self::OnTeam => true,
        }
    }

    /// Fixed candidate the grudge points at. Replaces the area search.
    /// Universal anger has no such target and still searches.
    fn grudge_target(self, mob: &dyn Mob) -> Option<Uuid> {
        match self {
            Self::AngryAt => mob
                .as_neutral()
                .and_then(NeutralMob::get_persistent_anger_target),
            Self::Always | Self::NoDaylight | Self::Adult | Self::Untamed | Self::OnTeam => None,
        }
    }

    /// Checked per candidate during the search.
    /// The grudge target is already filtered by the search.
    fn allows_target(self, mob: &dyn Mob, target: &dyn EntityBase, world: &World) -> bool {
        match self {
            Self::AngryAt => mob
                .as_neutral()
                .is_some_and(|neutral| neutral.is_angry_at(target, world)),
            Self::Always | Self::NoDaylight | Self::Adult | Self::Untamed | Self::OnTeam => true,
        }
    }
}

pub struct ActiveTargetGoal {
    track_target_goal: TrackTargetGoal,
    target: Option<Arc<dyn EntityBase>>,
    reciprocal_chance: i32,
    target_type: TargetMatch,
    target_predicate: TargetPredicate,
    condition: TargetCondition,
    gate: Option<MobFilter>,
    start_gate: Option<MobFilter>,
    target_filter: Option<MobFilter>,
    selector: Option<TargetPredicateFn>,
    search_box: Option<SearchBoxFn>,
}

/// Vanilla `getTargetSearchArea` overrides: the box non-player candidates are searched in,
/// from the hunter and its follow range.
pub type SearchBoxFn = fn(&dyn Mob, f64) -> BoundingBox;

impl ActiveTargetGoal {
    pub fn new<F>(
        mob: &MobEntity,
        target_type: impl Into<TargetMatch>,
        reciprocal_chance: i32,
        check_visibility: bool,
        check_can_navigate: bool,
        predicate: Option<F>,
    ) -> Self
    where
        F: Fn(&LivingEntity, &World) -> bool + Send + Sync + 'static,
    {
        let track_target_goal = TrackTargetGoal::new(check_visibility, check_can_navigate);
        let mut target_predicate = TargetPredicate::create_attackable();
        target_predicate.base_max_distance = mob
            .living_entity
            .get_attribute_value(&Attributes::FOLLOW_RANGE);

        if let Some(predicate) = predicate {
            target_predicate.set_predicate(predicate);
        }

        Self {
            track_target_goal,
            target: None,
            reciprocal_chance: to_goal_ticks(reciprocal_chance),
            target_type: target_type.into(),
            target_predicate,
            condition: TargetCondition::Always,
            gate: None,
            start_gate: None,
            target_filter: None,
            selector: None,
            search_box: None,
        }
    }

    /// Chains onto any of the constructors, including the boxed ones.
    #[must_use]
    pub fn when(mut self: Box<Self>, condition: TargetCondition) -> Box<Self> {
        self.condition = condition;
        self
    }

    /// Extra condition for starting and for continuing, e.g. an angry, unspent bee.
    #[must_use]
    pub fn gated_by(mut self: Box<Self>, gate: MobFilter) -> Box<Self> {
        self.gate = Some(gate);
        self
    }

    /// Extra condition for starting only, like a brain `StartAttacking` that runs in some
    /// activities and leaves the target to the others.
    #[must_use]
    pub fn start_gated_by(mut self: Box<Self>, gate: MobFilter) -> Box<Self> {
        self.start_gate = Some(gate);
        self
    }

    /// Only mob targets passing `filter`. Re-checked while continuing only with
    /// [`TargetCondition::Untamed`], which re-runs the whole search test.
    #[must_use]
    pub fn only_mobs(mut self: Box<Self>, filter: MobFilter) -> Box<Self> {
        self.target_filter = Some(filter);
        self
    }

    /// Search roughly once per `chance` ticks, 0 every tick.
    #[must_use]
    pub fn every(mut self: Box<Self>, chance: i32) -> Box<Self> {
        self.reciprocal_chance = to_goal_ticks(chance);
        self
    }

    /// Hunter-aware test on each candidate, checked only while searching.
    #[must_use]
    pub fn selecting(mut self: Box<Self>, selector: TargetPredicateFn) -> Box<Self> {
        self.selector = Some(selector);
        self
    }

    /// Searches non-player candidates in `search_box` instead of the follow range cube.
    #[must_use]
    pub fn searching_in(mut self: Box<Self>, search_box: SearchBoxFn) -> Box<Self> {
        self.search_box = Some(search_box);
        self
    }

    /// Ticks an unseen target is kept, vanilla `setUnseenMemoryTicks`.
    #[must_use]
    pub fn forget_unseen_after(mut self: Box<Self>, ticks: i32) -> Box<Self> {
        self.track_target_goal.max_time_without_visibility = ticks;
        self
    }

    /// Follow range factor for searching and keeping the target.
    #[must_use]
    pub fn scale_range(mut self: Box<Self>, scale: f64) -> Box<Self> {
        self.track_target_goal.follow_range_scale = scale;
        self
    }

    fn passes_filter(&self, target: &dyn EntityBase) -> bool {
        self.target_filter
            .is_none_or(|filter| target.get_mob().is_some_and(filter))
    }

    #[must_use]
    pub fn with_default(
        mob: &MobEntity,
        target_type: impl Into<TargetMatch>,
        check_visibility: bool,
    ) -> Box<Self> {
        let track_target_goal = TrackTargetGoal::with_default(check_visibility);
        let mut target_predicate = TargetPredicate::create_attackable();
        target_predicate.base_max_distance = mob
            .living_entity
            .get_attribute_value(&Attributes::FOLLOW_RANGE);

        Box::new(Self {
            track_target_goal,
            target: None,
            reciprocal_chance: to_goal_ticks(DEFAULT_RECIPROCAL_CHANCE),
            target_type: target_type.into(),
            target_predicate,
            condition: TargetCondition::Always,
            gate: None,
            start_gate: None,
            target_filter: None,
            selector: None,
            search_box: None,
        })
    }

    pub fn set_target(&mut self, target: Option<Arc<dyn EntityBase>>) {
        self.target = target;
    }

    /// Vanilla's targeting conditions plus the per-mob filters, cheap ones before the sight test.
    fn accepts(&self, mob: &dyn Mob, world: &World, candidate: &dyn EntityBase) -> bool {
        self.passes_filter(candidate)
            && self.selector.is_none_or(|selector| {
                candidate
                    .get_living_entity()
                    .is_some_and(|living| selector(mob, living, world))
            })
            && self.condition.allows_target(mob, candidate, world)
            && self.target_predicate.test(world, Some(mob), candidate)
    }

    fn find_closest_target(&mut self, mob: &dyn Mob) {
        let mob_entity = mob.get_mob_entity();
        let follow_range = self.track_target_goal.follow_range(mob_entity);

        // Vanilla updates the target conditions with the current follow distance on every search
        self.target_predicate.base_max_distance = follow_range;

        let entity = &mob_entity.living_entity.entity;
        let world = entity.world.load();
        let mut area = SearchArea::from_eyes(entity, follow_range, follow_range).players_anywhere();
        if let Some(search_box) = self.search_box {
            area.area = search_box(mob, follow_range);
        }

        // Pick the nearest candidate that passes the conditions, not the nearest overall.
        let target_type = self.target_type;
        let found = self.condition.grudge_target(mob).map_or_else(
            || {
                target_type.find_nearest(&world, &area, |candidate| {
                    self.accepts(mob, &world, candidate)
                })
            },
            // Same area rule as the search: players anywhere, others in the box.
            |uuid| {
                find_by_uuid(&world, uuid).filter(|candidate| {
                    target_type.matches(candidate.get_entity().entity_type)
                        && (candidate.get_player().is_some() || area.holds(candidate.as_ref()))
                        && self.accepts(mob, &world, candidate.as_ref())
                })
            },
        );

        self.target = found;
    }
}

impl Goal for ActiveTargetGoal {
    fn tick_reach(&self, mob: &dyn Mob, running: bool) -> Option<GoalReach> {
        let mob_entity = mob.get_mob_entity();
        let follow_range = self.track_target_goal.follow_range(mob_entity);
        let mut reach = GoalReach {
            links: self
                .target
                .iter()
                .map(|target| target.get_entity().entity_id)
                .chain(self.track_target_goal.target_mob_id())
                .collect(),
            sight: self.track_target_goal.target_mob_id().into_iter().collect(),
            ..GoalReach::default()
        };
        let world = mob.get_entity().world.load();
        let searches = !running
            && match self.condition {
                TargetCondition::NoDaylight => !mob_entity.is_in_daylight(),
                TargetCondition::OnTeam => mob.get_team_name().is_some(),
                // Only a grudge, or the universal anger an earlier goal or island member can
                // start this tick, lets the search run.
                TargetCondition::AngryAt => {
                    self.condition.allows_search(mob)
                        || world.level_info.load().game_rules.universal_anger
                }
                _ => true,
            };
        if searches && let Some(uuid) = self.condition.grudge_target(mob) {
            // The grudge target is looked up by UUID wherever it is.
            reach
                .links
                .extend(find_by_uuid(&world, uuid).map(|target| target.get_entity().entity_id));
        }
        if searches {
            // Sight checks on the candidates in the search box.
            reach.block_reads = follow_range;
            if self.target_type.matches(&EntityType::PLAYER) {
                reach.players = mob
                    .get_entity()
                    .world
                    .load()
                    .player_search
                    .attackable_reach(follow_range);
                reach.block_reads = reach.block_reads.max(reach.players);
            }
            if !self.target_type.is_player() {
                reach.typed.push((follow_range, self.target_type));
            }
        }
        Some(reach)
    }

    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        if self
            .gate
            .into_iter()
            .chain(self.start_gate)
            .any(|gate| !gate(mob))
        {
            return false;
        }
        // Vanilla `SpiderTargetGoal.canUse` checks the light before the search roll.
        if self.condition == TargetCondition::NoDaylight && mob.get_mob_entity().is_in_daylight() {
            return false;
        }
        // Vanilla `ShulkerDefenseAttackGoal.canUse` checks the team before the search roll.
        if self.condition == TargetCondition::OnTeam && mob.get_team_name().is_none() {
            return false;
        }
        if self.reciprocal_chance > 0
            && mob.get_random().random_range(0..self.reciprocal_chance) != 0
        {
            return false;
        }
        if !self.condition.allows_search(mob) {
            return false;
        }
        self.find_closest_target(mob);
        self.target.is_some()
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        if self.track_target_goal.was_cancelled() {
            return false;
        }
        if self.gate.is_some_and(|gate| !gate(mob)) {
            self.target = None;
            return false;
        }
        // Vanilla `NonTameRandomTargetGoal`: the search test again, so no unseen memory.
        if self.condition == TargetCondition::Untamed {
            let world = mob.get_mob_entity().living_entity.entity.world.load();
            return self
                .target
                .as_ref()
                .is_some_and(|target| self.accepts(mob, &world, target.as_ref()));
        }
        // Like vanilla TargetGoal.canContinueToUse, the condition only gates the search.
        self.track_target_goal.should_continue(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        let reason = if self.condition == TargetCondition::Untamed {
            TargetReason::RandomTarget
        } else if self
            .target
            .as_ref()
            .is_some_and(|t| t.get_player().is_some())
        {
            TargetReason::ClosestPlayer
        } else {
            TargetReason::ClosestEntity
        };
        self.track_target_goal
            .start_targeting(mob, self.target.clone(), reason);
        self.track_target_goal.start(mob);
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.track_target_goal.stop(mob);
    }

    fn controls(&self) -> Controls {
        self.track_target_goal.controls()
    }
}
