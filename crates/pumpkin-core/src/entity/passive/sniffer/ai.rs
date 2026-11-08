use pumpkin_data::entity::EntityType;
use pumpkin_data::environment_attribute::Activity;

use super::{SnifferEntity, SnifferState};
use crate::entity::ageable::AgeableMob;
use crate::entity::ai::brain::{
    Brain, BrainProvider, BrainTick,
    activity::ActivityData,
    behavior::{
        AnimalMakeLove, AnimalPanic, Behavior, BehaviorControl, CountDownCooldownTicks, DoNothing,
        FollowTemptation, LookAtTargetSink, MoveToTargetSink, Swim, Timed, random_stroll, run_one,
        set_entity_look_target, set_walk_target_from_look_target,
    },
    memory::{MemoryModuleId, MemoryStatus, PackedMemories, types, walk_target::WalkTarget},
    sensing::SensorType,
};
use crate::entity::mob::Mob;

const MAX_LOOK_DISTANCE: f32 = 6.0;
const SNIFFING_COOLDOWN_TICKS: i64 = 9600;
const SPEED_MULTIPLIER_WHEN_IDLING: f32 = 1.0;
const SPEED_MULTIPLIER_WHEN_PANICKING: f32 = 2.0;
const SPEED_MULTIPLIER_WHEN_SNIFFING: f32 = 1.25;
const SPEED_MULTIPLIER_WHEN_TEMPTED: f32 = 1.25;
const SEARCHING_TIME_OUT: i32 = 600;

const MEMORY_TYPES: [MemoryModuleId; 1] = [types::SNIFFER_EXPLORED_POSITIONS.id()];

pub(super) fn make_brain(sniffer: &SnifferEntity, packed: &PackedMemories) -> Brain {
    BrainProvider {
        memory_types: &MEMORY_TYPES,
        sensor_types: &[
            SensorType::NearestLivingEntities,
            SensorType::HurtBy,
            SensorType::NearestPlayers,
            SensorType::FoodTemptations,
        ],
        activities,
    }
    .make_brain(sniffer, packed)
}

pub(super) fn update_activity(brain: &mut Brain) {
    brain.set_active_activity_to_first_valid(&[Activity::Dig, Activity::Sniff, Activity::Idle]);
}

fn sniffer<'a>(tick: &BrainTick<'a>) -> Option<&'a SnifferEntity> {
    tick.mob.cast_any().downcast_ref::<SnifferEntity>()
}

/// Vanilla `SnifferAi.resetSniffing`.
fn reset_sniffing(tick: &mut BrainTick<'_>) {
    tick.state.erase(types::SNIFFER_DIGGING.id());
    tick.state.erase(types::SNIFFER_SNIFFING_TARGET.id());
    if let Some(sniffer) = sniffer(tick) {
        sniffer.transition_to(SnifferState::Idling);
    }
}

fn activities(_mob: &dyn Mob) -> Vec<ActivityData> {
    vec![
        core_activity(),
        idle_activity(),
        sniff_activity(),
        dig_activity(),
    ]
}

fn core_activity() -> ActivityData {
    ActivityData::with_priority_start(
        Activity::Core,
        0,
        vec![
            Box::new(Timed::new(Swim::new(0.8))),
            Box::new(Timed::new(ResetSniffingOnStart(AnimalPanic::new(
                SPEED_MULTIPLIER_WHEN_PANICKING,
            )))),
            Box::new(Timed::new(MoveToTargetSink::new(500, 700))),
            Box::new(CountDownCooldownTicks::new(
                types::TEMPTATION_COOLDOWN_TICKS,
            )),
        ],
    )
}

fn sniff_activity() -> ActivityData {
    ActivityData::with_pairs_and_conditions(
        Activity::Sniff,
        vec![(0, Box::new(Timed::new(Searching)))],
        vec![
            (types::IS_PANICKING.id(), MemoryStatus::ValueAbsent),
            (
                types::SNIFFER_SNIFFING_TARGET.id(),
                MemoryStatus::ValuePresent,
            ),
            (types::WALK_TARGET.id(), MemoryStatus::ValuePresent),
        ],
    )
}

fn dig_activity() -> ActivityData {
    ActivityData::with_pairs_and_conditions(
        Activity::Dig,
        vec![
            (0, Box::new(Timed::new(Digging))),
            (0, Box::new(Timed::new(FinishedDigging))),
        ],
        vec![
            (types::IS_PANICKING.id(), MemoryStatus::ValueAbsent),
            (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
            (types::SNIFFER_DIGGING.id(), MemoryStatus::ValuePresent),
        ],
    )
}

fn idle_activity() -> ActivityData {
    let behaviors: Vec<(i32, Box<dyn BehaviorControl>)> = vec![
        (
            0,
            Box::new(Timed::new(ResetSniffingOnStart(AnimalMakeLove::new(
                &EntityType::SNIFFER,
            )))),
        ),
        (
            1,
            Box::new(Timed::new(ResetSniffingOnStart(FollowTemptation::new(
                |_| SPEED_MULTIPLIER_WHEN_TEMPTED,
                |mob| {
                    if mob.get_mob_entity().living_entity.is_baby() {
                        2.5
                    } else {
                        3.5
                    }
                },
            )))),
        ),
        (2, Box::new(Timed::new(LookAtTargetSink::new(45, 90)))),
        (3, Box::new(Timed::new(FeelingHappy))),
        (
            4,
            Box::new(run_one(vec![
                (
                    Box::new(set_walk_target_from_look_target::create(
                        SPEED_MULTIPLIER_WHEN_IDLING,
                        3,
                    )),
                    2,
                ),
                (Box::new(Timed::new(Scenting)), 1),
                (Box::new(Timed::new(Sniffing)), 1),
                (
                    Box::new(set_entity_look_target::create(
                        |_, target| target.get_entity().entity_type == &EntityType::PLAYER,
                        MAX_LOOK_DISTANCE,
                    )),
                    1,
                ),
                (
                    Box::new(random_stroll::stroll(SPEED_MULTIPLIER_WHEN_IDLING)),
                    1,
                ),
                (Box::new(DoNothing::new(5, 20)), 2),
            ])),
        ),
    ];
    ActivityData::with_pairs_and_conditions(
        Activity::Idle,
        behaviors,
        vec![(types::SNIFFER_DIGGING.id(), MemoryStatus::ValueAbsent)],
    )
}

/// The anonymous subclasses in `SnifferAi` that reset sniffing before the behavior starts.
struct ResetSniffingOnStart<B: Behavior>(B);

impl<B: Behavior> Behavior for ResetSniffingOnStart<B> {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        self.0.entry_conditions()
    }

    fn min_duration(&self) -> i32 {
        self.0.min_duration()
    }

    fn max_duration(&self) -> i32 {
        self.0.max_duration()
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        self.0.check_extra_start_conditions(tick)
    }

    fn timed_out(&self, time: i64, end_timestamp: i64) -> bool {
        self.0.timed_out(time, end_timestamp)
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        reset_sniffing(tick);
        self.0.start(tick);
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        self.0.tick(tick);
    }

    fn stop_timed(&mut self, tick: &mut BrainTick<'_>, timed_out: bool) {
        self.0.stop_timed(tick, timed_out);
    }

    fn can_still_use(&mut self, tick: &mut BrainTick<'_>) -> bool {
        self.0.can_still_use(tick)
    }

    fn debug_name(&self) -> &'static str {
        self.0.debug_name()
    }
}

struct Digging;

impl Behavior for Digging {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        const CONDITIONS: [(MemoryModuleId, MemoryStatus); 4] = [
            (types::IS_PANICKING.id(), MemoryStatus::ValueAbsent),
            (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
            (types::SNIFFER_DIGGING.id(), MemoryStatus::ValuePresent),
            (types::SNIFF_COOLDOWN.id(), MemoryStatus::ValueAbsent),
        ];
        &CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        160
    }

    fn max_duration(&self) -> i32 {
        180
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        sniffer(tick).is_some_and(|sniffer| sniffer.can_sniff(tick.state))
    }

    fn can_still_use(&mut self, tick: &mut BrainTick<'_>) -> bool {
        sniffer(tick).is_some_and(|sniffer| {
            tick.state.has_memory_value(types::SNIFFER_DIGGING.id())
                && sniffer.can_dig(tick.state)
                && !sniffer.mob_entity.is_in_love()
        })
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        if let Some(sniffer) = sniffer(tick) {
            sniffer.transition_to(SnifferState::Digging);
        }
    }

    fn stop_timed(&mut self, tick: &mut BrainTick<'_>, timed_out: bool) {
        if timed_out {
            tick.state
                .set_with_expiry(types::SNIFF_COOLDOWN, (), SNIFFING_COOLDOWN_TICKS);
        } else {
            reset_sniffing(tick);
        }
    }

    fn debug_name(&self) -> &'static str {
        "Digging"
    }
}

struct FeelingHappy;

impl Behavior for FeelingHappy {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        const CONDITIONS: [(MemoryModuleId, MemoryStatus); 1] =
            [(types::SNIFFER_HAPPY.id(), MemoryStatus::ValuePresent)];
        &CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        40
    }

    fn max_duration(&self) -> i32 {
        100
    }

    fn can_still_use(&mut self, _tick: &mut BrainTick<'_>) -> bool {
        true
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        if let Some(sniffer) = sniffer(tick) {
            sniffer.transition_to(SnifferState::FeelingHappy);
        }
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        if let Some(sniffer) = sniffer(tick) {
            sniffer.transition_to(SnifferState::Idling);
        }
        tick.state.erase(types::SNIFFER_HAPPY.id());
    }

    fn debug_name(&self) -> &'static str {
        "FeelingHappy"
    }
}

struct FinishedDigging;

impl Behavior for FinishedDigging {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        const CONDITIONS: [(MemoryModuleId, MemoryStatus); 4] = [
            (types::IS_PANICKING.id(), MemoryStatus::ValueAbsent),
            (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
            (types::SNIFFER_DIGGING.id(), MemoryStatus::ValuePresent),
            (types::SNIFF_COOLDOWN.id(), MemoryStatus::ValuePresent),
        ];
        &CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        40
    }

    fn max_duration(&self) -> i32 {
        40
    }

    fn can_still_use(&mut self, tick: &mut BrainTick<'_>) -> bool {
        tick.state.has_memory_value(types::SNIFFER_DIGGING.id())
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        if let Some(sniffer) = sniffer(tick) {
            sniffer.transition_to(SnifferState::Rising);
        }
    }

    fn stop_timed(&mut self, tick: &mut BrainTick<'_>, timed_out: bool) {
        if let Some(sniffer) = sniffer(tick) {
            sniffer
                .transition_to(SnifferState::Idling)
                .on_digging_complete(tick.state, timed_out);
        }
        tick.state.erase(types::SNIFFER_DIGGING.id());
        tick.state.set(types::SNIFFER_HAPPY, true);
    }

    fn debug_name(&self) -> &'static str {
        "FinishedDigging"
    }
}

struct Scenting;

impl Behavior for Scenting {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        const CONDITIONS: [(MemoryModuleId, MemoryStatus); 5] = [
            (types::IS_PANICKING.id(), MemoryStatus::ValueAbsent),
            (types::SNIFFER_DIGGING.id(), MemoryStatus::ValueAbsent),
            (
                types::SNIFFER_SNIFFING_TARGET.id(),
                MemoryStatus::ValueAbsent,
            ),
            (types::SNIFFER_HAPPY.id(), MemoryStatus::ValueAbsent),
            (types::BREED_TARGET.id(), MemoryStatus::ValueAbsent),
        ];
        &CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        40
    }

    fn max_duration(&self) -> i32 {
        80
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        !SnifferEntity::is_tempted(tick.state)
    }

    fn can_still_use(&mut self, _tick: &mut BrainTick<'_>) -> bool {
        true
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        if let Some(sniffer) = sniffer(tick) {
            sniffer.transition_to(SnifferState::Scenting);
        }
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        if let Some(sniffer) = sniffer(tick) {
            sniffer.transition_to(SnifferState::Idling);
        }
    }

    fn debug_name(&self) -> &'static str {
        "Scenting"
    }
}

struct Searching;

impl Behavior for Searching {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        const CONDITIONS: [(MemoryModuleId, MemoryStatus); 3] = [
            (types::WALK_TARGET.id(), MemoryStatus::ValuePresent),
            (types::IS_PANICKING.id(), MemoryStatus::ValueAbsent),
            (
                types::SNIFFER_SNIFFING_TARGET.id(),
                MemoryStatus::ValuePresent,
            ),
        ];
        &CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        SEARCHING_TIME_OUT
    }

    fn max_duration(&self) -> i32 {
        SEARCHING_TIME_OUT
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        sniffer(tick).is_some_and(|sniffer| sniffer.can_sniff(tick.state))
    }

    fn can_still_use(&mut self, tick: &mut BrainTick<'_>) -> bool {
        let Some(sniffer) = sniffer(tick) else {
            return false;
        };
        if !sniffer.can_sniff(tick.state) {
            sniffer.transition_to(SnifferState::Idling);
            return false;
        }
        let walk_target = tick
            .state
            .get(types::WALK_TARGET)
            .map(|walk_target| walk_target.target().current_block_position());
        let sniffing_target = tick.state.get(types::SNIFFER_SNIFFING_TARGET).copied();
        walk_target.is_some() && walk_target == sniffing_target
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        if let Some(sniffer) = sniffer(tick) {
            sniffer.transition_to(SnifferState::Searching);
        }
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        if let Some(sniffer) = sniffer(tick)
            && sniffer.can_dig(tick.state)
            && sniffer.can_sniff(tick.state)
        {
            tick.state.set(types::SNIFFER_DIGGING, true);
        }
        tick.state.erase(types::WALK_TARGET.id());
        tick.state.erase(types::SNIFFER_SNIFFING_TARGET.id());
    }

    fn debug_name(&self) -> &'static str {
        "Searching"
    }
}

struct Sniffing;

impl Behavior for Sniffing {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        const CONDITIONS: [(MemoryModuleId, MemoryStatus); 3] = [
            (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
            (
                types::SNIFFER_SNIFFING_TARGET.id(),
                MemoryStatus::ValueAbsent,
            ),
            (types::SNIFF_COOLDOWN.id(), MemoryStatus::ValueAbsent),
        ];
        &CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        40
    }

    fn max_duration(&self) -> i32 {
        80
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        sniffer(tick).is_some_and(|sniffer| !sniffer.is_baby() && sniffer.can_sniff(tick.state))
    }

    fn can_still_use(&mut self, tick: &mut BrainTick<'_>) -> bool {
        sniffer(tick).is_some_and(|sniffer| sniffer.can_sniff(tick.state))
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        if let Some(sniffer) = sniffer(tick) {
            sniffer.transition_to(SnifferState::Sniffing);
        }
    }

    fn stop_timed(&mut self, tick: &mut BrainTick<'_>, timed_out: bool) {
        let Some(sniffer) = sniffer(tick) else {
            return;
        };
        sniffer.transition_to(SnifferState::Idling);
        if timed_out && let Some(position) = sniffer.calculate_dig_position(tick.state) {
            tick.state.set(types::SNIFFER_SNIFFING_TARGET, position);
            tick.state.set(
                types::WALK_TARGET,
                WalkTarget::from_block_pos(position, SPEED_MULTIPLIER_WHEN_SNIFFING, 0),
            );
        }
    }

    fn debug_name(&self) -> &'static str {
        "Sniffing"
    }
}
