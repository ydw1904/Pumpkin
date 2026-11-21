//! Vanilla `Level.blockEntityTickers` and `LevelChunk.tickersInLevel`: block entities tick one at
//! a time, in the order their positions first got a ticker.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector2::Vector2;
use rustc_hash::FxHashMap;

use crate::block::entities::BlockEntity;

/// [`BlockEntityTicker::removed`] of a ticker removed outside a block entity phase.
const REMOVED_OUTSIDE_PHASE: u64 = u64::MAX;

/// Vanilla `LevelChunk.RebindableTickingBlockEntityWrapper`: a position's place in the ticker
/// list, rebound to the position's next block entity until the position loses its block entity.
pub(super) struct BlockEntityTicker {
    block_entity: Mutex<Arc<dyn BlockEntity>>,
    /// 0 while live, else the phase it was removed in ([`REMOVED_OUTSIDE_PHASE`] between
    /// phases). Vanilla rebinds a removed wrapper to `NULL_TICKER`.
    removed: AtomicU64,
}

/// What a block entity phase finds in a ticker.
pub(super) enum TickerVisit {
    Live(Arc<dyn BlockEntity>),
    /// Removed earlier in this phase. Not ticked, but it was live when the phase began.
    RemovedThisPhase(Arc<dyn BlockEntity>),
    /// Removed before this phase; the list drops it.
    Removed,
}

impl BlockEntityTicker {
    pub(super) fn visit(&self, phase: u64) -> TickerVisit {
        let removed = self.removed.load(Ordering::Acquire);
        if removed != 0 && removed != phase {
            return TickerVisit::Removed;
        }
        let block_entity = self
            .block_entity
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if removed == 0 {
            TickerVisit::Live(block_entity)
        } else {
            TickerVisit::RemovedThisPhase(block_entity)
        }
    }
}

#[derive(Default)]
struct Tickers {
    /// Vanilla `Level.blockEntityTickers`. Taken out while a phase iterates it.
    in_level: Vec<Arc<BlockEntityTicker>>,
    /// Vanilla `Level.pendingBlockEntityTickers`: tickers added during a phase, appended at the
    /// start of the next one.
    pending: Vec<Arc<BlockEntityTicker>>,
    /// Vanilla `LevelChunk.tickersInLevel`, by chunk.
    by_chunk: FxHashMap<Vector2<i32>, FxHashMap<BlockPos, Arc<BlockEntityTicker>>>,
    /// The running phase, or 0 between phases (vanilla `tickingBlockEntities`).
    phase: u64,
    last_phase: u64,
}

impl Tickers {
    const fn removal_mark(&self) -> u64 {
        if self.phase == 0 {
            REMOVED_OUTSIDE_PHASE
        } else {
            self.phase
        }
    }
}

#[derive(Default)]
pub(super) struct BlockEntityTickers {
    tickers: Mutex<Tickers>,
}

/// The tickers a block entity phase iterates; hand them back with
/// [`BlockEntityTickers::end_phase`].
pub(super) struct TickPhase {
    pub(super) tickers: Vec<Arc<BlockEntityTicker>>,
    pub(super) phase: u64,
}

impl BlockEntityTickers {
    fn lock(&self) -> std::sync::MutexGuard<'_, Tickers> {
        self.tickers.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Vanilla `LevelChunk.updateBlockEntityTicker`: a position that still has a ticker keeps
    /// its place and ticks the new block entity (`rebind`); any other gets a new last one.
    pub(super) fn add(&self, block_entity: &Arc<dyn BlockEntity>) {
        let position = block_entity.get_position();
        let mut guard = self.lock();
        let tickers = &mut *guard;
        let chunk = tickers
            .by_chunk
            .entry(position.chunk_position())
            .or_default();
        if let Some(ticker) = chunk.get(&position) {
            *ticker
                .block_entity
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = block_entity.clone();
            return;
        }
        let ticker = Arc::new(BlockEntityTicker {
            block_entity: Mutex::new(block_entity.clone()),
            removed: AtomicU64::new(0),
        });
        chunk.insert(position, ticker.clone());
        // Vanilla `Level.addBlockEntityTicker`.
        if tickers.phase == 0 {
            tickers.in_level.push(ticker);
        } else {
            tickers.pending.push(ticker);
        }
    }

    /// Vanilla `LevelChunk.removeBlockEntityTicker`.
    pub(super) fn remove(&self, position: &BlockPos) {
        let mut tickers = self.lock();
        let chunk_pos = position.chunk_position();
        let Some(chunk) = tickers.by_chunk.get_mut(&chunk_pos) else {
            return;
        };
        let Some(ticker) = chunk.remove(position) else {
            return;
        };
        if chunk.is_empty() {
            tickers.by_chunk.remove(&chunk_pos);
        }
        ticker
            .removed
            .store(tickers.removal_mark(), Ordering::Release);
    }

    /// Vanilla `LevelChunk.clearAllBlockEntities` on unload: a reloaded chunk's tickers go to
    /// the end.
    pub(super) fn remove_chunk(&self, chunk_pos: Vector2<i32>) {
        let mut tickers = self.lock();
        let Some(chunk) = tickers.by_chunk.remove(&chunk_pos) else {
            return;
        };
        let mark = tickers.removal_mark();
        for ticker in chunk.values() {
            ticker.removed.store(mark, Ordering::Release);
        }
    }

    /// The start of vanilla `Level.tickBlockEntities`: appends the pending tickers and sends
    /// later ones to pending until [`Self::end_phase`].
    pub(super) fn begin_phase(&self) -> TickPhase {
        let mut guard = self.lock();
        let tickers = &mut *guard;
        tickers.last_phase += 1;
        tickers.phase = tickers.last_phase;
        let mut in_level = std::mem::take(&mut tickers.in_level);
        in_level.append(&mut tickers.pending);
        TickPhase {
            tickers: in_level,
            phase: tickers.phase,
        }
    }

    /// Puts the list back once the phase dropped its removed tickers.
    pub(super) fn end_phase(&self, tickers: Vec<Arc<BlockEntityTicker>>) {
        let mut guard = self.lock();
        guard.in_level = tickers;
        guard.phase = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::entities::block_entity_from_nbt;
    use pumpkin_nbt::compound::NbtCompound;

    fn chest(x: i32) -> Arc<dyn BlockEntity> {
        let mut nbt = NbtCompound::new();
        nbt.put_string("id", "minecraft:chest".to_string());
        nbt.put_int("x", x);
        nbt.put_int("y", 64);
        nbt.put_int("z", 0);
        block_entity_from_nbt(&nbt).unwrap()
    }

    /// One phase, recording what it visits like `World::tick` and keeping the live tickers.
    fn run_phase(
        tickers: &BlockEntityTickers,
        mut during: impl FnMut(i32),
    ) -> (Vec<i32>, Vec<i32>) {
        let TickPhase {
            tickers: mut list,
            phase,
        } = tickers.begin_phase();
        let mut ticked = Vec::new();
        let mut seen = Vec::new();
        list.retain(|ticker| match ticker.visit(phase) {
            TickerVisit::Live(be) => {
                let x = be.get_position().0.x;
                ticked.push(x);
                seen.push(x);
                during(x);
                true
            }
            TickerVisit::RemovedThisPhase(be) => {
                seen.push(be.get_position().0.x);
                false
            }
            TickerVisit::Removed => false,
        });
        tickers.end_phase(list);
        (ticked, seen)
    }

    #[test]
    fn tickers_keep_vanilla_list_order() {
        let tickers = BlockEntityTickers::default();
        for x in 0..4 {
            tickers.add(&chest(x));
        }
        // Rebinding keeps the place, a removed position goes to the end when it comes back.
        tickers.add(&chest(1));
        tickers.remove(&BlockPos::new(0, 64, 0));
        tickers.add(&chest(0));
        assert_eq!(run_phase(&tickers, |_| {}).0, [1, 2, 3, 0]);

        // Added during a phase: pending, so after the ones added before the next phase.
        // Removed during a phase before its turn: skipped, but still seen once.
        let (ticked, seen) = run_phase(&tickers, |x| {
            if x == 1 {
                tickers.add(&chest(10));
                tickers.remove(&BlockPos::new(3, 64, 0));
            }
        });
        assert_eq!(ticked, [1, 2, 0]);
        assert_eq!(seen, [1, 2, 3, 0]);
        tickers.add(&chest(11));
        assert_eq!(run_phase(&tickers, |_| {}).1, [1, 2, 0, 11, 10]);

        // Removed and added again in one phase: the old ticker is dropped, the new one waits.
        let (ticked, _) = run_phase(&tickers, |x| {
            if x == 1 {
                tickers.remove(&BlockPos::new(2, 64, 0));
                tickers.add(&chest(2));
            }
        });
        assert_eq!(ticked, [1, 0, 11, 10]);

        // Unloading drops the chunk's tickers, pending ones too; a reload goes to the end.
        tickers.add(&chest(20));
        tickers.remove_chunk(Vector2::new(0, 0));
        tickers.add(&chest(1));
        assert_eq!(run_phase(&tickers, |_| {}).1, [20, 1]);
    }
}
