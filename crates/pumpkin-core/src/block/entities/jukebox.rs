use std::any::Any;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use pumpkin_data::data_component_impl::JukeboxPlayableImpl;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::jukebox_song::JukeboxSong;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::position::BlockPos;

use crate::block::entities::BlockEntity;
use crate::world::World;
use pumpkin_inventory::{Clearable, Inventory};

/// Matches vanilla's `JukeboxBlockEntity`
pub struct JukeboxBlockEntity {
    position: BlockPos,
    world: OnceLock<Weak<World>>,
    /// The record item stored in the jukebox (`RecordItem` in NBT)
    record_stack: Arc<Mutex<ItemStack>>,
    /// Ticks since the current song started playing
    ticks_since_song_started: AtomicU64,
    /// Length of the current song in ticks (0 if not playing)
    song_length_ticks: AtomicU64,
    dirty: AtomicBool,
    comparator_dirty: AtomicBool,
}

const RECORD_ITEM_NBT_KEY: &str = "RecordItem";
const TICKS_SINCE_SONG_STARTED_NBT_KEY: &str = "ticks_since_song_started";

impl BlockEntity for JukeboxBlockEntity {
    fn resource_location(&self) -> &'static str {
        Self::ID
    }

    fn get_position(&self) -> BlockPos {
        self.position
    }

    fn from_nbt(nbt: &NbtCompound, position: BlockPos) -> Self
    where
        Self: Sized,
    {
        let record_stack = nbt
            .get_compound(RECORD_ITEM_NBT_KEY)
            .and_then(ItemStack::read_item_stack)
            .unwrap_or_else(|| ItemStack::EMPTY.clone());

        let ticks_since_song_started = nbt
            .get_long(TICKS_SINCE_SONG_STARTED_NBT_KEY)
            .and_then(|ticks| u64::try_from(ticks).ok());
        let song_length_ticks = ticks_since_song_started
            .and_then(|ticks| {
                Self::song_length(&record_stack)
                    .filter(|length| ticks < *length)
                    .map(|length| (ticks, length))
            })
            .unwrap_or((0, 0));

        Self {
            position,
            world: OnceLock::new(),
            record_stack: Arc::new(Mutex::new(record_stack)),
            ticks_since_song_started: AtomicU64::new(song_length_ticks.0),
            song_length_ticks: AtomicU64::new(song_length_ticks.1),
            dirty: AtomicBool::new(false),
            comparator_dirty: AtomicBool::new(false),
        }
    }

    fn write_nbt(&self, nbt: &mut NbtCompound) {
        let record = self
            .record_stack
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !record.is_empty() {
            let mut record_nbt = NbtCompound::new();
            record.write_item_stack(&mut record_nbt);
            nbt.put(RECORD_ITEM_NBT_KEY, record_nbt);
        }

        let ticks = self.ticks_since_song_started.load(Ordering::Relaxed);
        if self.song_length_ticks.load(Ordering::Relaxed) > 0 {
            nbt.put_long(TICKS_SINCE_SONG_STARTED_NBT_KEY, ticks as i64);
        }
    }

    fn tick(&self, world: &Arc<World>) {
        // Increment ticks if we're playing
        self.world.get_or_init(|| Arc::downgrade(world));
        let song_length = self.song_length_ticks.load(Ordering::Relaxed);
        if song_length > 0 {
            let ticks = self
                .ticks_since_song_started
                .fetch_add(1, Ordering::Relaxed);
            // Check if song has finished
            if ticks >= song_length {
                self.stop_playing();
                // TODO: Update block state to has_record = false? Or just stop redstone?
                // In vanilla, the disc stays but music stops and redstone turns off
            } else if ticks.is_multiple_of(20) {
                world.emit_game_event(
                    pumpkin_data::game_event::GameEvent::JukeboxPlay.name(),
                    self.position.to_centered_f64(),
                );
            }
        }
    }

    fn is_comparator_dirty(&self) -> bool {
        self.comparator_dirty.load(Ordering::Relaxed)
    }

    fn clear_comparator_dirty(&self) {
        self.comparator_dirty.store(false, Ordering::Relaxed);
    }

    fn is_dirty(&self) -> bool {
        self.dirty.load(Ordering::Relaxed)
    }

    fn bedrock_block_actor_data(
        &self,
        _state_id: pumpkin_data::BlockStateId,
    ) -> Option<NbtCompound> {
        let record_guard = self.record_stack.lock().ok();
        let record = record_guard.as_deref();
        Some(build_jukebox_actor(
            self.position,
            record,
            self.ticks_since_song_started.load(Ordering::Relaxed),
        ))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn get_inventory(self: Arc<Self>) -> Option<Arc<dyn Inventory>> {
        Some(self)
    }
}

impl JukeboxBlockEntity {
    pub const ID: &'static str = "minecraft:jukebox";

    #[must_use]
    pub fn new(position: BlockPos) -> Self {
        Self {
            position,
            world: OnceLock::new(),
            record_stack: Arc::new(Mutex::new(ItemStack::EMPTY.clone())),
            ticks_since_song_started: AtomicU64::new(0),
            song_length_ticks: AtomicU64::new(0),
            dirty: AtomicBool::new(false),
            comparator_dirty: AtomicBool::new(false),
        }
    }

    fn song_length(record: &ItemStack) -> Option<u64> {
        let playable = record.get_data_component::<JukeboxPlayableImpl>()?;
        let song_name = playable.song.rsplit(':').next()?;
        JukeboxSong::from_name(song_name).map(|song| song.length_in_ticks())
    }

    /// Get the current record stack
    pub fn get_record(&self) -> ItemStack {
        self.record_stack
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Set the record stack - matches vanilla's `setStack()`
    /// Note: The caller is responsible for updating block state and playing music
    pub fn set_record(&self, stack: ItemStack) {
        *self
            .record_stack
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = stack;
        self.mark_dirty();
    }

    /// Clear the stack and return what was there - used for dropping
    pub fn clear_record(&self) -> ItemStack {
        self.stop_playing();
        let mut record = self
            .record_stack
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let taken = record.clone();
        *record = ItemStack::EMPTY.clone();
        self.mark_dirty();
        taken
    }

    /// Start playing a song with the given length in ticks
    pub fn start_playing(&self, length_in_ticks: u64) {
        self.ticks_since_song_started.store(0, Ordering::Relaxed);
        self.song_length_ticks
            .store(length_in_ticks, Ordering::Relaxed);
        self.mark_dirty();
    }

    /// Stop playing the current song
    pub fn stop_playing(&self) {
        self.ticks_since_song_started.store(0, Ordering::Relaxed);
        let playing = self.song_length_ticks.swap(0, Ordering::Relaxed) != 0;
        if playing && let Some(world) = self.world.get().and_then(Weak::upgrade) {
            world.emit_game_event(
                pumpkin_data::game_event::GameEvent::JukeboxStopPlay.name(),
                self.position.to_centered_f64(),
            );
        }
        self.mark_dirty();
    }

    /// Check if a song is currently playing
    pub fn is_playing(&self) -> bool {
        let song_length = self.song_length_ticks.load(Ordering::Relaxed);
        if song_length == 0 {
            return false;
        }
        let ticks = self.ticks_since_song_started.load(Ordering::Relaxed);
        ticks < song_length
    }

    fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::Relaxed);
        self.comparator_dirty.store(true, Ordering::Relaxed);
    }
}

/// Implements single-slot inventory for jukebox (matches vanilla's `SingleStackInventory`)
impl Inventory for JukeboxBlockEntity {
    fn size(&self) -> usize {
        1
    }

    fn is_empty(&self) -> bool {
        self.record_stack
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    }

    fn get_stack(&self, _slot: usize) -> ItemStack {
        self.record_stack
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn remove_stack(&self, _slot: usize) -> ItemStack {
        self.stop_playing();
        let mut record = self
            .record_stack
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let taken = record.clone();
        *record = ItemStack::EMPTY.clone();
        self.mark_dirty();
        taken
    }

    fn remove_stack_specific(&self, _slot: usize, _amount: u8) -> ItemStack {
        // Jukebox only holds one item, so remove the whole stack
        self.remove_stack(0)
    }

    fn set_stack(&self, _slot: usize, stack: ItemStack) {
        *self
            .record_stack
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = stack;
        self.mark_dirty();
    }

    fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::Relaxed);
        self.comparator_dirty.store(true, Ordering::Relaxed);
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl Clearable for JukeboxBlockEntity {
    fn clear(&self) {
        self.stop_playing();
        *self
            .record_stack
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = ItemStack::EMPTY.clone();
        self.mark_dirty();
    }
}

/// Builds the Bedrock block-actor NBT for jukeboxes.
#[must_use]
pub fn build_jukebox_actor(
    position: BlockPos,
    record: Option<&ItemStack>,
    ticks_since_song_started: u64,
) -> NbtCompound {
    let mut builder = super::bedrock::BedrockBlockActorBuilder::new("Jukebox", position)
        .put_long("TicksSinceSongStarted", ticks_since_song_started as i64);

    if let Some(record_stack) = record
        && !record_stack.is_empty()
    {
        let mut rec_nbt = NbtCompound::new();
        rec_nbt.put_string(
            "Name",
            super::bedrock::bedrock_item_name(record_stack.item.registry_key),
        );
        rec_nbt.put_byte("Count", 1);
        builder = builder.put_compound("RecordItem", rec_nbt);
    }

    builder.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::{Entity, ai::test_support::world, passive::allay::AllayEntity};
    use pumpkin_data::{entity::EntityType, item::Item};
    use pumpkin_util::math::{vector2::Vector2, vector3::Vector3};

    #[test]
    fn jukebox_actor_builder() {
        let pos = BlockPos::new(8, 64, 8);
        let record = ItemStack::new(1, &Item::MUSIC_DISC_CAT);
        let nbt = build_jukebox_actor(pos, Some(&record), 100);
        assert_eq!(nbt.get_string("id"), Some("Jukebox"));
        assert_eq!(nbt.get_long("TicksSinceSongStarted"), Some(100));
        let rec = nbt.get_compound("RecordItem").expect("RecordItem missing");
        assert_eq!(rec.get_string("Name"), Some("minecraft:music_disc_cat"));
    }

    #[tokio::test]
    async fn removing_the_record_through_inventory_stops_allay_dancing()
    -> Result<(), Box<dyn std::error::Error>> {
        let folder = tempfile::tempdir()?;
        let world = world(&folder);
        world.level.loaded_chunks.insert(
            Vector2::new(0, 0),
            pumpkin_world::chunk::ChunkData::empty_sync(0, 0),
        );
        let allay = AllayEntity::new(Entity::new(
            world.clone(),
            Vector3::new(1.5, 64.0, 0.5),
            &EntityType::ALLAY,
        ));
        world.add_entity_silent(allay.clone());
        let jukebox = JukeboxBlockEntity::new(BlockPos::new(0, 64, 0));
        jukebox.start_playing(100);
        jukebox.tick(&world);
        assert!(allay.is_dancing());
        jukebox.remove_stack(0);
        assert!(!allay.is_dancing());
        world.level.shutdown().await;
        Ok(())
    }

    fn jukebox_nbt(ticks: Option<i64>) -> NbtCompound {
        let mut record = NbtCompound::new();
        ItemStack::new(1, &Item::MUSIC_DISC_PIGSTEP).write_item_stack(&mut record);

        let mut nbt = NbtCompound::new();
        nbt.put_compound(RECORD_ITEM_NBT_KEY, record);
        if let Some(ticks) = ticks {
            nbt.put_long(TICKS_SINCE_SONG_STARTED_NBT_KEY, ticks);
        }
        nbt
    }

    #[test]
    fn restores_active_song_from_nbt() {
        let entity = JukeboxBlockEntity::from_nbt(&jukebox_nbt(Some(20)), BlockPos::new(0, 64, 0));

        assert!(entity.is_playing());
        assert_eq!(entity.ticks_since_song_started.load(Ordering::Relaxed), 20);
        assert_eq!(
            entity.song_length_ticks.load(Ordering::Relaxed),
            JukeboxSong::Pigstep.length_in_ticks()
        );
    }

    #[test]
    fn does_not_start_song_without_playback_ticks() {
        let entity = JukeboxBlockEntity::from_nbt(&jukebox_nbt(None), BlockPos::new(0, 64, 0));

        assert!(!entity.is_playing());
    }

    #[test]
    fn preserves_song_started_at_tick_zero() {
        let entity = JukeboxBlockEntity::new(BlockPos::new(0, 64, 0));
        entity.set_record(ItemStack::new(1, &Item::MUSIC_DISC_PIGSTEP));
        entity.start_playing(JukeboxSong::Pigstep.length_in_ticks());

        let mut nbt = NbtCompound::new();
        entity.write_nbt(&mut nbt);
        let restored = JukeboxBlockEntity::from_nbt(&nbt, BlockPos::new(0, 64, 0));

        assert!(restored.is_playing());
        assert_eq!(restored.ticks_since_song_started.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn does_not_restore_finished_song() {
        let length = JukeboxSong::Pigstep.length_in_ticks();
        let entity = JukeboxBlockEntity::from_nbt(
            &jukebox_nbt(Some(length as i64)),
            BlockPos::new(0, 64, 0),
        );

        assert!(!entity.is_playing());
        assert_eq!(entity.song_length_ticks.load(Ordering::Relaxed), 0);
    }
}
