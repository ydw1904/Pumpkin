//! Randomness for gameplay (entities, block ticks, commands), as opposed to world generation.
//!
//! Draws normally come from a per-thread [`LegacyRand`], the algorithm behind vanilla's
//! `RandomSource.create()`. Setting `PUMPKIN_RNG_SEED` turns on the RNG lockdown used by the
//! parity harness: every draw comes from a stream owned by whatever is running (an entity, a
//! scheduled tick, a chunk, or `misc`), seeded from the run seed and the owner's key, and
//! `PUMPKIN_RNG_LOG` writes each draw to a file. The harness's Java agent does the same for
//! vanilla, so the two servers' random outcomes can be compared draw by draw.

use std::{
    cell::RefCell,
    collections::HashMap,
    fs::File,
    io::{BufWriter, Write},
    sync::{
        Arc, Mutex, OnceLock, PoisonError,
        atomic::{AtomicI32, Ordering},
    },
};

use uuid::Uuid;

use super::{RandomImpl, get_seed, legacy_rand::LegacyRand};
use crate::math::java_string_hash;

/// Owner of draws made outside any owner scope.
const MISC_OWNER: &str = "misc";
/// Log line written by [`reset`]; the harness only compares draws after the last one.
const RESET_MARK: &str = "#reset";
/// Player keys are `player:<name>`, without the `#<n>` other entities get.
const PLAYER_KEY_PREFIX: &str = "player:";

thread_local! {
    static LOCAL: RefCell<LegacyRand> = RefCell::new(LegacyRand::from_seed(get_seed()));
    static OWNERS: RefCell<Vec<Arc<str>>> = const { RefCell::new(Vec::new()) };
}

struct Lockdown {
    seed: i64,
    /// Resets so far. Mixed into stream seeds so a restarted stream doesn't replay values drawn
    /// before the reset, such as an existing entity's UUID.
    epoch: AtomicI32,
    streams: Mutex<HashMap<Arc<str>, LegacyRand>>,
    entity_keys: Mutex<HashMap<i32, Arc<str>>>,
    key_counts: Mutex<HashMap<String, u32>>,
    log: Option<Mutex<BufWriter<File>>>,
}

fn lockdown() -> Option<&'static Lockdown> {
    static LOCKDOWN: OnceLock<Option<Lockdown>> = OnceLock::new();
    LOCKDOWN
        .get_or_init(|| {
            let seed = std::env::var("PUMPKIN_RNG_SEED")
                .ok()?
                .trim()
                .parse()
                .ok()?;
            let log = std::env::var_os("PUMPKIN_RNG_LOG")
                .and_then(|path| File::create(path).ok())
                .map(|file| Mutex::new(BufWriter::new(file)));
            Some(Lockdown {
                seed,
                epoch: AtomicI32::new(0),
                streams: Mutex::default(),
                entity_keys: Mutex::default(),
                key_counts: Mutex::default(),
                log,
            })
        })
        .as_ref()
}

/// Whether the RNG lockdown is on. Owner keys only need building when it is.
#[must_use]
pub fn is_locked_down() -> bool {
    lockdown().is_some()
}

impl Lockdown {
    fn draw<T>(
        &self,
        method: &str,
        arg: Option<i32>,
        draw: impl FnOnce(&mut LegacyRand) -> T,
        format: impl FnOnce(&T) -> String,
    ) -> T {
        let owner = OWNERS
            .with_borrow(|owners| owners.last().cloned())
            .unwrap_or_else(|| Arc::from(MISC_OWNER));
        let value = {
            let mut streams = self.streams.lock().unwrap_or_else(PoisonError::into_inner);
            let stream = streams.entry(owner.clone()).or_insert_with(|| {
                // Java: `new LegacyRandomSource(seed ^ (long) key.hashCode() ^ ((long) epoch << 32))`.
                let epoch = i64::from(self.epoch.load(Ordering::Relaxed)) << 32;
                LegacyRand::from_seed(
                    (self.seed ^ i64::from(java_string_hash(&owner)) ^ epoch) as u64,
                )
            });
            draw(stream)
        };
        if let Some(log) = &self.log {
            let arg = arg.map_or_else(|| "-".to_string(), |arg| arg.to_string());
            let mut log = log.lock().unwrap_or_else(PoisonError::into_inner);
            // Flushed per line: the server is usually stopped without running destructors.
            let _ = writeln!(log, "{owner}\t{method}\t{arg}\t{}", format(&value));
            let _ = log.flush();
        }
        value
    }

    fn entity_key(&self, entity_id: i32, base: impl FnOnce() -> String) -> Arc<str> {
        let mut keys = self
            .entity_keys
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        keys.entry(entity_id)
            .or_insert_with(|| {
                let base = base();
                if base.starts_with(PLAYER_KEY_PREFIX) {
                    return Arc::from(base);
                }
                let mut counts = self
                    .key_counts
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                let count = counts.entry(base.clone()).or_default();
                let key = format!("{base}#{count}");
                *count += 1;
                Arc::from(key)
            })
            .clone()
    }
}

/// Restarts every stream and the entity numbering, called when `/tick freeze` freezes the game,
/// so a scene built while frozen starts from the same state on both servers. Marks the log.
pub fn reset() {
    let Some(lockdown) = lockdown() else {
        return;
    };
    // Under the streams lock, so no stream is created with the old epoch after the clear.
    let mut streams = lockdown
        .streams
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    lockdown.epoch.fetch_add(1, Ordering::Relaxed);
    streams.clear();
    drop(streams);
    lockdown
        .key_counts
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clear();
    if let Some(log) = &lockdown.log {
        let mut log = log.lock().unwrap_or_else(PoisonError::into_inner);
        let _ = writeln!(log, "{RESET_MARK}");
        let _ = log.flush();
    }
}

/// Runs `f` with draws owned by `key`, e.g. `block@1,2,3` for a scheduled block tick.
/// `key` is only built while the RNG lockdown is on.
pub fn with_owner<R>(key: impl FnOnce() -> String, f: impl FnOnce() -> R) -> R {
    if lockdown().is_none() {
        return f();
    }
    let _owner = OwnerScope::enter(key());
    f()
}

/// Runs `f` with draws owned by an entity. The entity's key is `base` plus `#<n>`, the number
/// of entities registered with the same `base` before it; see [`register_entity`].
pub fn with_entity_owner<R>(
    entity_id: i32,
    base: impl FnOnce() -> String,
    f: impl FnOnce() -> R,
) -> R {
    let Some(lockdown) = lockdown() else {
        return f();
    };
    let _owner = OwnerScope::enter(lockdown.entity_key(entity_id, base));
    f()
}

/// Gives an entity its owner key when it joins the world, so keys don't depend on tick order.
/// `base` is `<type>@<x>,<y>,<z>` for its block position, or `player:<name>` for players.
pub fn register_entity(entity_id: i32, base: impl FnOnce() -> String) {
    if let Some(lockdown) = lockdown() {
        lockdown.entity_key(entity_id, base);
    }
}

/// Makes draws belong to `owner` until dropped. Prefer [`with_owner`]; this is for loops where
/// a closure doesn't fit. Only create it while [`is_locked_down`].
pub struct OwnerScope;

impl OwnerScope {
    #[must_use]
    pub fn enter(owner: impl Into<Arc<str>>) -> Self {
        OWNERS.with_borrow_mut(|owners| owners.push(owner.into()));
        Self
    }
}

impl Drop for OwnerScope {
    fn drop(&mut self) {
        OWNERS.with_borrow_mut(Vec::pop);
    }
}

/// The random source for gameplay code. It has vanilla `RandomSource`'s methods, under the
/// names [`RandomImpl`] uses.
#[derive(Clone, Copy, Default)]
pub struct GameRandom;

/// Returns the gameplay random source. Vanilla's entity and level `RandomSource`s both map to it.
#[must_use]
pub const fn game_random() -> GameRandom {
    GameRandom
}

fn draw<T>(
    method: &str,
    arg: Option<i32>,
    draw: impl FnOnce(&mut LegacyRand) -> T,
    format: impl FnOnce(&T) -> String,
) -> T {
    match lockdown() {
        Some(lockdown) => lockdown.draw(method, arg, draw, format),
        None => LOCAL.with_borrow_mut(draw),
    }
}

impl GameRandom {
    #[must_use]
    pub fn next_i32(self) -> i32 {
        draw("nextInt", None, RandomImpl::next_i32, i32::to_string)
    }

    #[must_use]
    pub fn next_bounded_i32(self, bound: i32) -> i32 {
        draw(
            "nextIntBounded",
            Some(bound),
            |rng| rng.next_bounded_i32(bound),
            i32::to_string,
        )
    }

    /// Vanilla `RandomSource.nextIntBetweenInclusive`.
    #[must_use]
    pub fn next_inbetween_i32(self, min: i32, max: i32) -> i32 {
        self.next_bounded_i32(max - min + 1) + min
    }

    /// Vanilla `RandomSource.nextInt(origin, bound)`.
    #[must_use]
    pub fn next_inbetween_i32_exclusive(self, min: i32, max: i32) -> i32 {
        min + self.next_bounded_i32(max - min)
    }

    #[must_use]
    pub fn next_i64(self) -> i64 {
        draw("nextLong", None, RandomImpl::next_i64, i64::to_string)
    }

    #[must_use]
    pub fn next_bool(self) -> bool {
        draw("nextBoolean", None, RandomImpl::next_bool, bool::to_string)
    }

    #[must_use]
    pub fn next_f32(self) -> f32 {
        draw("nextFloat", None, RandomImpl::next_f32, |value| {
            format!("0x{:08x}", value.to_bits())
        })
    }

    #[must_use]
    pub fn next_f64(self) -> f64 {
        draw("nextDouble", None, RandomImpl::next_f64, |value| {
            format!("0x{:016x}", value.to_bits())
        })
    }

    #[must_use]
    pub fn next_gaussian(self) -> f64 {
        draw("nextGaussian", None, RandomImpl::next_gaussian, |value| {
            format!("0x{:016x}", value.to_bits())
        })
    }

    /// Vanilla `Mth.nextFloat`: `min` without a draw when the range is empty.
    #[must_use]
    pub fn next_inbetween_f32(self, min: f32, max: f32) -> f32 {
        if min >= max {
            return min;
        }
        self.next_f32() * (max - min) + min
    }

    /// Vanilla `Mth.nextDouble`: `min` without a draw when the range is empty.
    #[must_use]
    pub fn next_inbetween_f64(self, min: f64, max: f64) -> f64 {
        if min >= max {
            return min;
        }
        self.next_f64() * (max - min) + min
    }

    /// Vanilla `RandomSource.triangle(double, double)`.
    #[allow(clippy::suboptimal_flops)]
    #[must_use]
    pub fn next_triangular(self, mode: f64, deviation: f64) -> f64 {
        mode + deviation * (self.next_f64() - self.next_f64())
    }

    /// Vanilla `RandomSource.consumeCount`.
    pub fn skip(self, count: i32) {
        for _ in 0..count {
            let _ = self.next_i32();
        }
    }

    /// Vanilla `Mth.createInsecureUUID`, which gives every new entity its UUID.
    #[must_use]
    pub fn create_insecure_uuid(self) -> Uuid {
        // Version 4 and the IETF variant, like `Mth.createInsecureUUID`.
        let most = (self.next_i64() & !0xF000) | 0x4000;
        let least = (self.next_i64() & 0x3FFF_FFFF_FFFF_FFFF) | i64::MIN;
        Uuid::from_u64_pair(most as u64, least as u64)
    }

    /// Vanilla `RandomSource.fork` on a legacy source: an independent stream seeded by one draw.
    #[must_use]
    pub fn split(self) -> LegacyRand {
        LegacyRand::from_seed(self.next_i64() as u64)
    }
}
