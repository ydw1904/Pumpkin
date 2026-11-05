use super::super::BrainTick;
use super::super::memory::{MemoryModuleId, MemoryModuleType, types};
use super::Sensor;
use crate::entity::EntityBase;

const REQUIRES: [MemoryModuleId; 1] = [types::NEAREST_LIVING_ENTITIES.id()];

pub type MobTest = fn(&BrainTick<'_>, &dyn EntityBase) -> bool;
pub type ReadyTest = fn(&BrainTick<'_>) -> bool;

/// Vanilla `MobSensor`: sets `to_set` for a while when a nearby mob passes `mob_test`, and
/// erases it while the body isn't ready.
pub struct MobSensor {
    mob_test: MobTest,
    ready_test: ReadyTest,
    to_set: MemoryModuleType<bool>,
    memory_time_to_live: i64,
}

impl MobSensor {
    #[must_use]
    pub const fn new(
        mob_test: MobTest,
        ready_test: ReadyTest,
        to_set: MemoryModuleType<bool>,
        memory_time_to_live: i64,
    ) -> Self {
        Self {
            mob_test,
            ready_test,
            to_set,
            memory_time_to_live,
        }
    }
}

impl Sensor for MobSensor {
    fn requires(&self) -> &'static [MemoryModuleId] {
        &REQUIRES
    }

    fn do_tick(&mut self, tick: &mut BrainTick<'_>) {
        if !(self.ready_test)(tick) {
            tick.state.erase(self.to_set.id());
            return;
        }
        let mob_present = tick
            .state
            .get(types::NEAREST_LIVING_ENTITIES)
            .is_some_and(|entities| {
                entities
                    .iter()
                    .any(|entity| (self.mob_test)(tick, entity.as_ref()))
            });
        if mob_present {
            tick.state
                .set_with_expiry(self.to_set, true, self.memory_time_to_live);
        }
    }
}
