use super::super::memory::{MemoryModuleId, MemoryStatus, types};
use super::OneShot;

/// Vanilla `BecomePassiveIfMemoryPresent.create`: while `pacifying_memory` is present, the mob
/// forgets its attack target and stays `PACIFIED` for `pacify_duration` ticks.
#[must_use]
pub fn create(pacifying_memory: MemoryModuleId, pacify_duration: i64) -> OneShot {
    OneShot::new(
        "BecomePassiveIfMemoryPresent",
        vec![
            (types::ATTACK_TARGET.id(), MemoryStatus::Registered),
            (types::PACIFIED.id(), MemoryStatus::ValueAbsent),
            (pacifying_memory, MemoryStatus::ValuePresent),
        ],
        move |tick| {
            tick.state
                .set_with_expiry(types::PACIFIED, true, pacify_duration);
            tick.state.erase(types::ATTACK_TARGET.id());
            true
        },
    )
}
