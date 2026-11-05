use std::collections::BTreeMap;
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};

use pumpkin_util::math::position::BlockPos;
use rustc_hash::FxHashSet;

use crate::tick::{OrderedTick, ScheduledTick, TICK_QUEUE_SIZE};

pub struct ChunkTickScheduler<T> {
    inner: Mutex<Option<Box<ChunkTickSchedulerInner<T>>>>,
    /// The number of the latest `step_tick`. A tick scheduled with delay `d` is due on step
    /// `offset + d` and runs on the `d`th step from now, like vanilla's `gameTime + delay`.
    offset: AtomicUsize,
}

struct ChunkTickSchedulerInner<T> {
    tick_queue: [Vec<OrderedTick<T>>; TICK_QUEUE_SIZE],
    /// Ticks too far ahead for `tick_queue`, keyed by the step they are due on.
    far_ticks: BTreeMap<usize, Vec<OrderedTick<T>>>,
    queued_ticks: FxHashSet<(BlockPos, T)>,
}

impl<T> ChunkTickSchedulerInner<T> {
    fn new() -> Self {
        Self {
            tick_queue: std::array::from_fn(|_| Vec::new()),
            far_ticks: BTreeMap::new(),
            queued_ticks: FxHashSet::default(),
        }
    }
}

impl<'a, T: std::hash::Hash + Eq> ChunkTickScheduler<&'a T> {
    pub fn step_tick(&self) -> Vec<OrderedTick<&'a T>> {
        // Advance before taking the slot, so ticks scheduled while these run count from this
        // step rather than the next one.
        let current_step = self.offset.load(Ordering::SeqCst) + 1;
        self.offset.store(current_step, Ordering::SeqCst);

        let mut inner_guard = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(inner) = inner_guard.as_mut() else {
            return Vec::new();
        };

        while let Some(entry) = inner.far_ticks.first_entry()
            && *entry.key() < current_step + TICK_QUEUE_SIZE
        {
            let due_step = *entry.key();
            inner.tick_queue[due_step % TICK_QUEUE_SIZE].append(&mut entry.remove());
        }

        let res = std::mem::take(&mut inner.tick_queue[current_step % TICK_QUEUE_SIZE]);

        if !res.is_empty() {
            for next_tick in &res {
                inner
                    .queued_ticks
                    .remove(&(next_tick.position, next_tick.value));
            }
            if inner.queued_ticks.is_empty() {
                *inner_guard = None;
            }
        }
        res
    }

    /// Queues the tick unless the same one is already queued, and returns whether it was.
    pub fn schedule_tick(&self, tick: &ScheduledTick<&'a T>, sub_tick_order: u64) -> bool {
        let offset = self.offset.load(Ordering::SeqCst);
        let mut inner_guard = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let inner = inner_guard.get_or_insert_with(|| Box::new(ChunkTickSchedulerInner::new()));

        if inner.queued_ticks.insert((tick.position, tick.value)) {
            // Vanilla runs a zero-delay tick on the next tick, since the current one has
            // already collected its ticks.
            let delay = tick.delay.max(1) as usize;
            let due_step = offset + delay;
            let ordered_tick = OrderedTick {
                priority: tick.priority,
                sub_tick_order,
                position: tick.position,
                value: tick.value,
            };
            if delay < TICK_QUEUE_SIZE {
                inner.tick_queue[due_step % TICK_QUEUE_SIZE].push(ordered_tick);
            } else {
                inner
                    .far_ticks
                    .entry(due_step)
                    .or_default()
                    .push(ordered_tick);
            }
            true
        } else {
            false
        }
    }

    pub fn is_scheduled(&self, pos: BlockPos, value: &T) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .is_some_and(|inner| inner.queued_ticks.contains(&(pos, value)))
    }

    pub fn clear_area(&self, min: &BlockPos, max: &BlockPos) {
        let mut inner_guard = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(inner) = inner_guard.as_mut() else {
            return;
        };

        let contains = |position: &BlockPos| {
            position.0.x >= min.0.x
                && position.0.x < max.0.x
                && position.0.y >= min.0.y
                && position.0.y < max.0.y
                && position.0.z >= min.0.z
                && position.0.z < max.0.z
        };

        for queue in &mut inner.tick_queue {
            queue.retain(|tick| !contains(&tick.position));
        }
        inner.far_ticks.retain(|_, queue| {
            queue.retain(|tick| !contains(&tick.position));
            !queue.is_empty()
        });
        inner
            .queued_ticks
            .retain(|(position, _)| !contains(position));
        let became_empty = inner.queued_ticks.is_empty();

        if became_empty {
            *inner_guard = None;
        }
    }

    pub fn has_ticks(&self) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .is_some_and(|inner| !inner.queued_ticks.is_empty())
    }

    #[must_use]
    pub fn to_vec(&self) -> Vec<ScheduledTick<&'a T>> {
        self.to_vec_with_order()
            .into_iter()
            .map(|(tick, _)| tick)
            .collect()
    }

    /// Like [`Self::to_vec`], with each tick's sub-tick order.
    #[must_use]
    pub fn to_vec_with_order(&self) -> Vec<(ScheduledTick<&'a T>, u64)> {
        let offset = self.offset.load(Ordering::SeqCst);
        let inner_guard = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(inner) = inner_guard.as_ref() else {
            return Vec::new();
        };

        let mut res = Vec::new();

        for i in 0..TICK_QUEUE_SIZE {
            let index = (offset + i) % TICK_QUEUE_SIZE;
            res.extend(inner.tick_queue[index].iter().map(|x| {
                let tick = ScheduledTick {
                    delay: i as u32,
                    priority: x.priority,
                    position: x.position,
                    value: x.value,
                };
                (tick, x.sub_tick_order)
            }));
        }
        for (due_step, queue) in &inner.far_ticks {
            res.extend(queue.iter().map(|x| {
                let tick = ScheduledTick {
                    delay: (due_step - offset) as u32,
                    priority: x.priority,
                    position: x.position,
                    value: x.value,
                };
                (tick, x.sub_tick_order)
            }));
        }
        res
    }
}

impl<'a, T: std::hash::Hash + Eq + 'static> FromIterator<ScheduledTick<&'a T>>
    for ChunkTickScheduler<&'a T>
{
    fn from_iter<I: IntoIterator<Item = ScheduledTick<&'a T>>>(iter: I) -> Self {
        let scheduler = Self::default();
        let iter = iter.into_iter();

        let (lower, _) = iter.size_hint();
        if lower > 0 {
            let mut inner_guard = scheduler
                .inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let inner = inner_guard.get_or_insert_with(|| Box::new(ChunkTickSchedulerInner::new()));
            inner.queued_ticks.reserve(lower);
        }

        for tick in iter {
            scheduler.schedule_tick(&tick, 0);
        }
        scheduler
    }
}

impl<T> Default for ChunkTickScheduler<T> {
    fn default() -> Self {
        Self {
            inner: Mutex::new(None),
            offset: AtomicUsize::new(0),
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::tick::TickPriority;

    static VALUE: u8 = 0;

    fn tick(delay: u32) -> ScheduledTick<&'static u8> {
        ScheduledTick {
            delay,
            priority: TickPriority::Normal,
            position: BlockPos::new(0, 0, 0),
            value: &VALUE,
        }
    }

    /// Steps until the scheduled tick comes out and returns how many steps that took.
    fn steps_until_run(scheduler: &ChunkTickScheduler<&'static u8>) -> usize {
        (1..=20_000)
            .find(|_| !scheduler.step_tick().is_empty())
            .unwrap()
    }

    #[test]
    fn tick_scheduled_while_ticking_runs_after_its_delay() {
        let scheduler = ChunkTickScheduler::default();
        scheduler.step_tick();
        // Scheduled after this step took its ticks, like a repeater scheduling the next one.
        scheduler.schedule_tick(&tick(8), 0);
        assert_eq!(steps_until_run(&scheduler), 8);
    }

    #[test]
    fn zero_delay_runs_on_the_next_step() {
        let scheduler = ChunkTickScheduler::default();
        scheduler.step_tick();
        scheduler.schedule_tick(&tick(0), 0);
        assert_eq!(steps_until_run(&scheduler), 1);
    }

    #[test]
    fn saved_ticks_keep_their_remaining_delay() {
        let scheduler = ChunkTickScheduler::default();
        scheduler.step_tick();
        scheduler.schedule_tick(&tick(5), 0);
        scheduler.step_tick();
        let saved = scheduler.to_vec();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].delay, 4);
        let loaded: ChunkTickScheduler<&'static u8> = saved.into_iter().collect();
        assert_eq!(steps_until_run(&loaded), 4);
    }

    #[test]
    fn delays_past_the_ring_run_on_time_and_survive_a_reload() {
        for delay in [255, 256, 3600] {
            let scheduler = ChunkTickScheduler::default();
            scheduler.step_tick();
            scheduler.schedule_tick(&tick(delay), 0);
            assert_eq!(steps_until_run(&scheduler), delay as usize);
        }

        let scheduler = ChunkTickScheduler::default();
        scheduler.step_tick();
        scheduler.schedule_tick(&tick(3600), 0);
        for _ in 0..100 {
            scheduler.step_tick();
        }
        let saved = scheduler.to_vec();
        assert_eq!(saved[0].delay, 3500);
        let loaded: ChunkTickScheduler<&'static u8> = saved.into_iter().collect();
        assert_eq!(steps_until_run(&loaded), 3500);
    }
}
