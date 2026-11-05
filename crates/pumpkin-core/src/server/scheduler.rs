use crate::server::Server;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, Ordering};

/// Where vanilla's `TimerQueue` is saved: `data/minecraft/scheduled_events.dat`.
pub const SAVED_DATA_PATH: &str = "scheduled_events";

/// Vanilla has no limit, but a corrupt `scheduled_events.dat` shouldn't load without bound.
const MAX_SAVED_EVENTS: usize = 1 << 20;

const FUNCTION_CALLBACK: &str = "minecraft:function";
const FUNCTION_TAG_CALLBACK: &str = "minecraft:function_tag";

/// Vanilla `FunctionCallback` and `FunctionTagCallback`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScheduledCallback {
    Function(String),
    FunctionTag(String),
}

impl ScheduledCallback {
    /// The name `DatapackManager` resolves: `ns:path`, or `#ns:path` for a tag.
    fn function_name(&self) -> String {
        match self {
            Self::Function(id) => id.clone(),
            Self::FunctionTag(id) => format!("#{id}"),
        }
    }

    fn to_nbt(&self) -> NbtCompound {
        let (callback_type, id) = match self {
            Self::Function(id) => (FUNCTION_CALLBACK, id),
            Self::FunctionTag(id) => (FUNCTION_TAG_CALLBACK, id),
        };
        let mut nbt = NbtCompound::new();
        nbt.put_string("type", callback_type.to_string());
        nbt.put_string("id", id.clone());
        nbt
    }

    fn from_nbt(nbt: &NbtCompound) -> Option<Self> {
        let id = nbt.get_string("id")?.to_string();
        match nbt.get_string("type")? {
            FUNCTION_CALLBACK => Some(Self::Function(id)),
            FUNCTION_TAG_CALLBACK => Some(Self::FunctionTag(id)),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
struct ScheduledEvent {
    id: String,
    callback: ScheduledCallback,
}

#[derive(Default)]
struct Events {
    /// Ordered like vanilla's queue: by trigger time, then by the order they were scheduled.
    queue: BTreeMap<(i64, u64), ScheduledEvent>,
    sequential_id: u64,
    /// The event whose function [`ScheduledFunctionQueue::tick`] is running. Autosave runs
    /// beside the tick, so a save taken meanwhile keeps it unless it already rescheduled itself.
    running: Option<(i64, ScheduledEvent)>,
}

/// Vanilla `TimerQueue<MinecraftServer>`: functions scheduled by `/schedule`, keyed by the
/// overworld's game time.
pub struct ScheduledFunctionQueue {
    events: Mutex<Events>,
    /// The game time [`Self::tick`] is running events for, or `-1` outside it.
    ticking_time: AtomicI64,
}

impl Default for ScheduledFunctionQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl ScheduledFunctionQueue {
    #[must_use]
    pub fn new() -> Self {
        Self {
            events: Mutex::new(Events::default()),
            ticking_time: AtomicI64::new(-1),
        }
    }

    fn events(&self) -> std::sync::MutexGuard<'_, Events> {
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The game time `/schedule` counts from. Vanilla runs scheduled functions after
    /// `ServerLevel.tickTime` has advanced the game time, which Pumpkin's world tick only does
    /// after this queue ticks, so a function scheduling another sees the advanced time too.
    #[must_use]
    pub fn current_game_time(&self, world_game_time: i64) -> i64 {
        let ticking = self.ticking_time.load(Ordering::Relaxed);
        if ticking >= 0 {
            ticking
        } else {
            world_game_time
        }
    }

    /// Vanilla `TimerQueue.schedule`: an event with the same id and time is only kept once.
    pub fn schedule(&self, id: String, time: i64, callback: ScheduledCallback) {
        let mut events = self.events();
        if events
            .queue
            .range((time, 0)..=(time, u64::MAX))
            .any(|(_, event)| event.id == id)
        {
            return;
        }
        events.sequential_id += 1;
        let key = (time, events.sequential_id);
        events.queue.insert(key, ScheduledEvent { id, callback });
    }

    /// Vanilla `TimerQueue.remove`: removes every event with this id and returns how many.
    pub fn remove(&self, id: &str) -> usize {
        let mut events = self.events();
        let before = events.queue.len();
        events.queue.retain(|_, event| event.id != id);
        before - events.queue.len()
    }

    #[must_use]
    pub fn get_event_ids(&self) -> Vec<String> {
        let events = self.events();
        let mut ids: Vec<String> = events.queue.values().map(|e| e.id.clone()).collect();
        ids.sort();
        ids.dedup();
        ids
    }

    /// Vanilla `TimerQueue.tick`: runs every event due at `current_time`, in order. An event is
    /// removed before its function runs, so a function that reschedules itself stays queued.
    pub fn tick(&self, server: &Arc<Server>, current_time: i64) {
        self.ticking_time.store(current_time, Ordering::Relaxed);
        loop {
            let event = {
                let mut events = self.events();
                match events.queue.first_key_value() {
                    Some((&(time, _), _)) if time <= current_time => {
                        let event = events.queue.pop_first().map(|(_, event)| event);
                        events.running = event.clone().map(|event| (time, event));
                        event
                    }
                    _ => None,
                }
            };
            let Some(event) = event else {
                break;
            };
            let source = crate::command::CommandSender::Console
                .into_source(server)
                .with_silent();
            // A function that no longer exists does nothing, like vanilla's callbacks.
            let _ = server.datapack_manager.execute_function(
                server,
                &source,
                &event.callback.function_name(),
            );
            self.events().running = None;
        }
        self.ticking_time.store(-1, Ordering::Relaxed);
    }

    /// Vanilla `TimerQueue.pack`, the `data` compound of `scheduled_events.dat`.
    #[must_use]
    pub fn to_saved_data(&self) -> NbtCompound {
        let events = self.events();
        let running = events
            .running
            .as_ref()
            .filter(|(_, running)| !events.queue.values().any(|event| event.id == running.id));
        let list = running
            .map(|(time, event)| (*time, event))
            .into_iter()
            .chain(events.queue.iter().map(|(&(time, _), event)| (time, event)))
            .map(|(time, event)| {
                let mut nbt = NbtCompound::new();
                nbt.put_long("trigger_time", time);
                nbt.put_string("id", event.id.clone());
                nbt.put_compound("callback", event.callback.to_nbt());
                NbtTag::Compound(nbt)
            })
            .collect();
        let mut data = NbtCompound::new();
        data.put("events", NbtTag::List(list));
        data
    }

    /// Restores the queue from `scheduled_events.dat`. Vanilla's codec drops events it can't
    /// read; events already due run on the next tick.
    pub fn load_saved_data(&self, data: &NbtCompound) {
        let Some(list) = data.get_list("events") else {
            return;
        };
        for event in list.iter().take(MAX_SAVED_EVENTS) {
            let NbtTag::Compound(event) = event else {
                continue;
            };
            let (Some(time), Some(id), Some(callback)) = (
                event.get_long("trigger_time"),
                event.get_string("id"),
                event
                    .get_compound("callback")
                    .and_then(ScheduledCallback::from_nbt),
            ) else {
                continue;
            };
            self.schedule(id.to_string(), time, callback);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ScheduledCallback, ScheduledFunctionQueue};

    fn ids_in_order(queue: &ScheduledFunctionQueue) -> Vec<(i64, String)> {
        queue
            .events()
            .queue
            .iter()
            .map(|(&(time, _), event)| (time, event.id.clone()))
            .collect()
    }

    #[test]
    fn same_time_events_keep_their_order_and_duplicates_are_dropped() {
        let queue = ScheduledFunctionQueue::new();
        let function = |id: &str| ScheduledCallback::Function(id.to_string());
        queue.schedule("ns:b".into(), 10, function("ns:b"));
        queue.schedule("ns:a".into(), 10, function("ns:a"));
        queue.schedule("ns:b".into(), 10, function("ns:b"));
        queue.schedule("ns:b".into(), 5, function("ns:b"));
        assert_eq!(
            ids_in_order(&queue),
            [(5, "ns:b".into()), (10, "ns:b".into()), (10, "ns:a".into())]
        );
        assert_eq!(queue.remove("ns:b"), 2);
        assert_eq!(ids_in_order(&queue), [(10, "ns:a".into())]);
    }

    #[test]
    fn saved_events_load_in_vanilla_format() {
        let queue = ScheduledFunctionQueue::new();
        queue.schedule(
            "#ns:loop".into(),
            120,
            ScheduledCallback::FunctionTag("ns:loop".into()),
        );
        queue.schedule(
            "ns:once".into(),
            100,
            ScheduledCallback::Function("ns:once".into()),
        );
        let saved = queue.to_saved_data();
        let first = saved
            .get_list("events")
            .and_then(|list| list.first())
            .cloned();
        let Some(pumpkin_nbt::tag::NbtTag::Compound(first)) = first else {
            panic!("no events saved");
        };
        assert_eq!(first.get_long("trigger_time"), Some(100));
        assert_eq!(first.get_string("id"), Some("ns:once"));
        let callback = first.get_compound("callback");
        assert_eq!(
            callback.and_then(|c| c.get_string("type")),
            Some("minecraft:function")
        );

        let restored = ScheduledFunctionQueue::new();
        restored.load_saved_data(&saved);
        assert_eq!(
            ids_in_order(&restored),
            [(100, "ns:once".into()), (120, "#ns:loop".into())]
        );
    }
}
