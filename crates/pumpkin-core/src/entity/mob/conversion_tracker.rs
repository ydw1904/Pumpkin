use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use pumpkin_data::tracked_data::TrackedData;
use pumpkin_nbt::compound::NbtCompound;

use crate::entity::mob::MobEntity;

const NOT_CONVERTING: i32 = -1;

/// Vanilla `ConversionTracker`: a mob that stays afflicted (a zombie under water, a skeleton in
/// powder snow) for long enough starts converting, and converts once that runs out.
pub struct ConversionTracker {
    converting_data: TrackedData,
    affliction_tag: &'static str,
    total_affliction_time: i32,
    conversion_tag: &'static str,
    total_conversion_time: i32,
    affliction_time: AtomicI32,
    conversion_time: AtomicI32,
    converting: AtomicBool,
}

impl ConversionTracker {
    #[must_use]
    pub const fn new(
        converting_data: TrackedData,
        affliction_tag: &'static str,
        total_affliction_time: i32,
        conversion_tag: &'static str,
        total_conversion_time: i32,
    ) -> Self {
        Self {
            converting_data,
            affliction_tag,
            total_affliction_time,
            conversion_tag,
            total_conversion_time,
            affliction_time: AtomicI32::new(0),
            conversion_time: AtomicI32::new(NOT_CONVERTING),
            converting: AtomicBool::new(false),
        }
    }

    /// Runs `convert` once the conversion finishes.
    pub fn tick(&self, mob: &MobEntity, afflicted: bool, convert: impl FnOnce()) {
        if !mob.living_entity.is_alive() || mob.is_no_ai() {
            return;
        }
        if afflicted {
            if self.is_converting() {
                if self.conversion_time.fetch_sub(1, Ordering::Relaxed) - 1 < 0 {
                    convert();
                }
            } else if self.affliction_time.fetch_add(1, Ordering::Relaxed) + 1
                >= self.total_affliction_time
            {
                self.start_conversion(mob, self.total_conversion_time);
            }
        } else {
            self.affliction_time.store(-1, Ordering::Relaxed);
            self.set_converting(mob, false);
        }
    }

    /// Whether `tick` converts the mob if it stays afflicted.
    #[must_use]
    pub fn converts_next_tick(&self) -> bool {
        self.is_converting() && self.conversion_time.load(Ordering::Relaxed) <= 0
    }

    #[must_use]
    pub fn is_converting(&self) -> bool {
        self.converting.load(Ordering::Relaxed)
    }

    fn set_converting(&self, mob: &MobEntity, converting: bool) {
        if self.converting.swap(converting, Ordering::Relaxed) != converting {
            mob.living_entity
                .entity
                .set_synced_data(self.converting_data, converting);
        }
    }

    fn start_conversion(&self, mob: &MobEntity, time: i32) {
        self.conversion_time.store(time, Ordering::Relaxed);
        self.set_converting(mob, true);
    }

    pub fn write_nbt(&self, nbt: &mut NbtCompound, afflicted: bool) {
        nbt.put_int(
            self.conversion_tag,
            if self.is_converting() {
                self.conversion_time.load(Ordering::Relaxed)
            } else {
                NOT_CONVERTING
            },
        );
        nbt.put_int(
            self.affliction_tag,
            if afflicted {
                self.affliction_time.load(Ordering::Relaxed)
            } else {
                -1
            },
        );
    }

    pub fn read_nbt(&self, mob: &MobEntity, nbt: &NbtCompound) {
        self.affliction_time.store(
            nbt.get_int(self.affliction_tag).unwrap_or(0),
            Ordering::Relaxed,
        );
        match nbt.get_int(self.conversion_tag) {
            Some(time) if time != NOT_CONVERTING => self.start_conversion(mob, time),
            _ => self.set_converting(mob, false),
        }
    }
}
