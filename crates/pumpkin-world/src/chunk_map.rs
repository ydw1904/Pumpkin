use std::cell::RefCell;
use std::ops::Deref;
use std::sync::atomic::{AtomicU64, Ordering};

use dashmap::DashMap;
use pumpkin_util::math::vector2::Vector2;
use rustc_hash::FxBuildHasher;

use crate::level::SyncChunk;

static NEXT_MAP_ID: AtomicU64 = AtomicU64::new(0);

/// The loaded chunks of a level.
///
/// Every block read looks its chunk up here, so the map hashes with Fx instead of `SipHash`, and
/// [`Self::read`] keeps the last chunks each thread read, like the last chunk cache of vanilla
/// `ServerChunkCache.getChunk`.
pub struct ChunkMap {
    chunks: DashMap<Vector2<i32>, SyncChunk, FxBuildHasher>,
    id: u64,
    /// Changes on every insert and remove, which invalidates the cached chunks.
    generation: AtomicU64,
}

struct RecentChunk {
    map: u64,
    generation: u64,
    position: Vector2<i32>,
    chunk: SyncChunk,
}

thread_local! {
    static RECENT_CHUNKS: RefCell<[Option<RecentChunk>; 2]> = const { RefCell::new([None, None]) };
}

impl Default for ChunkMap {
    fn default() -> Self {
        Self {
            chunks: DashMap::default(),
            id: NEXT_MAP_ID.fetch_add(1, Ordering::Relaxed),
            generation: AtomicU64::new(0),
        }
    }
}

impl Deref for ChunkMap {
    type Target = DashMap<Vector2<i32>, SyncChunk, FxBuildHasher>;

    fn deref(&self) -> &Self::Target {
        &self.chunks
    }
}

impl ChunkMap {
    pub fn insert(&self, position: Vector2<i32>, chunk: SyncChunk) -> Option<SyncChunk> {
        let previous = self.chunks.insert(position, chunk);
        self.generation.fetch_add(1, Ordering::Release);
        previous
    }

    pub fn remove(&self, position: &Vector2<i32>) -> Option<(Vector2<i32>, SyncChunk)> {
        let removed = self.chunks.remove(position);
        self.generation.fetch_add(1, Ordering::Release);
        removed
    }

    /// Runs `f` on the chunk at `position`, if it is loaded.
    pub fn read<R>(&self, position: &Vector2<i32>, f: impl FnOnce(&SyncChunk) -> R) -> Option<R> {
        let generation = self.generation.load(Ordering::Acquire);
        let is_entry = |entry: &RecentChunk| {
            entry.map == self.id && entry.generation == generation && entry.position == *position
        };
        let mut f = Some(f);
        // `f` runs inside the borrow, so a read it makes itself finds the cache borrowed and
        // goes to the map.
        let cached = RECENT_CHUNKS
            .try_with(|recent| {
                let recent = recent.try_borrow().ok()?;
                let entry = recent.iter().flatten().find(|entry| is_entry(entry))?;
                f.take().map(|f| f(&entry.chunk))
            })
            .ok()
            .flatten();
        if cached.is_some() {
            return cached;
        }
        let f = f?;
        let chunk = self.chunks.get(position)?.value().clone();
        let result = f(&chunk);
        let _ = RECENT_CHUNKS.try_with(|recent| {
            if let Ok(mut recent) = recent.try_borrow_mut() {
                recent[1] = recent[0].take();
                recent[0] = Some(RecentChunk {
                    map: self.id,
                    generation,
                    position: *position,
                    chunk,
                });
            }
        });
        Some(result)
    }
}

#[cfg(test)]
mod tests {
    use super::ChunkMap;
    use crate::chunk::ChunkData;
    use pumpkin_util::math::vector2::Vector2;
    use std::sync::Arc;

    #[test]
    fn reads_see_chunks_inserted_and_removed_after_a_cached_read() {
        let map = ChunkMap::default();
        let position = Vector2::new(0, 0);
        let first = ChunkData::empty_sync(0, 0);
        map.insert(position, first.clone());
        assert_eq!(
            map.read(&position, |chunk| Arc::ptr_eq(chunk, &first)),
            Some(true)
        );

        let second = ChunkData::empty_sync(0, 0);
        map.insert(position, second.clone());
        assert_eq!(
            map.read(&position, |chunk| Arc::ptr_eq(chunk, &second)),
            Some(true)
        );

        map.remove(&position);
        assert!(map.read(&position, |_| ()).is_none());
    }
}
