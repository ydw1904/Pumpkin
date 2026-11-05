use std::collections::VecDeque;

use pumpkin_util::math::vector2::Vector2;
use rustc_hash::{FxHashMap, FxHashSet};
use uuid::Uuid;

/// Vanilla `DistanceManager.naturalSpawnChunkCounter` range, in chunks around a player's chunk.
const NATURAL_SPAWN_CHUNK_RANGE: i32 = 8;

/// The chunks a world simulates, split like vanilla's simulation chunk levels.
#[derive(Clone, Default)]
pub struct ActiveChunks {
    /// Vanilla `inEntityTickingRange`: within simulation distance of a player, or a forced chunk.
    /// Entities tick and chunks get random ticks and precipitation here.
    pub entity_ticking: FxHashSet<Vector2<i32>>,
    /// Vanilla `inBlockTickingRange`: one ring further than `entity_ticking`. Scheduled ticks,
    /// block events and block entities run here.
    pub block_ticking: FxHashSet<Vector2<i32>>,
    /// Vanilla `ChunkHolder.getTickingChunk() != null`: block ticking chunks whose
    /// `prepareTickingChunk` task has post-processed them. Only these get scheduled and random
    /// ticks, spawn mobs and tick their entities.
    pub ticking: FxHashSet<Vector2<i32>>,
    /// Vanilla `getSpawnCandidateChunks`: chunks within eight chunks of a player's chunk. Their
    /// count is the mob cap's chunk count.
    pub spawn_candidates: FxHashSet<Vector2<i32>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct ActivePlayerArea {
    pub center: Vector2<i32>,
    pub simulation_distance: i32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Square {
    center: Vector2<i32>,
    radius: i32,
}

impl Square {
    const fn contains(self, pos: Vector2<i32>) -> bool {
        (pos.x - self.center.x).abs() <= self.radius && (pos.y - self.center.y).abs() <= self.radius
    }

    fn chunks(self) -> impl Iterator<Item = Vector2<i32>> {
        pumpkin_data::chunk_view_lut::get_chebyshev_square(self.radius.max(0) as u8)
            .iter()
            .map(move |&(dx, dz)| self.center.add_raw(i32::from(dx), i32::from(dz)))
    }
}

/// Union of squares, counting how many squares cover each chunk.
#[derive(Default)]
struct SquareUnion {
    counts: FxHashMap<Vector2<i32>, u32>,
}

impl SquareUnion {
    fn add(&mut self, pos: Vector2<i32>, chunks: &mut FxHashSet<Vector2<i32>>) -> bool {
        let count = self.counts.entry(pos).or_default();
        *count += 1;
        *count == 1 && chunks.insert(pos)
    }

    fn remove(&mut self, pos: Vector2<i32>, chunks: &mut FxHashSet<Vector2<i32>>) -> bool {
        let Some(count) = self.counts.get_mut(&pos) else {
            return false;
        };
        *count -= 1;
        if *count > 0 {
            return false;
        }
        self.counts.remove(&pos);
        chunks.remove(&pos)
    }

    /// Replaces `from` with `to`, touching only the chunks that differ.
    fn update(
        &mut self,
        from: Option<Square>,
        to: Option<Square>,
        chunks: &mut FxHashSet<Vector2<i32>>,
        mut on_added: impl FnMut(Vector2<i32>),
        mut on_removed: impl FnMut(Vector2<i32>),
    ) {
        if from == to {
            return;
        }
        if let Some(from) = from {
            for pos in from.chunks() {
                if !to.is_some_and(|to| to.contains(pos)) && self.remove(pos, chunks) {
                    on_removed(pos);
                }
            }
        }
        if let Some(to) = to {
            for pos in to.chunks() {
                if !from.is_some_and(|from| from.contains(pos)) && self.add(pos, chunks) {
                    on_added(pos);
                }
            }
        }
    }
}

#[derive(Default)]
pub(super) struct ActiveChunkTracker {
    pub players: FxHashMap<Uuid, ActivePlayerArea>,
    /// Block ticking chunks that were loaded when they last became block ticking.
    pub loaded_block_ticking_chunks: FxHashSet<Vector2<i32>>,
    /// Loaded block ticking chunks waiting for their `prepareTickingChunk` task, in the order
    /// vanilla would queue them on the main thread executor.
    pub ticking_tasks: VecDeque<Vector2<i32>>,
    /// Queued chunks whose neighbours aren't all loaded yet. Vanilla's ticking future waits for
    /// the chunks around it to be full before its task is queued.
    pub ticking_tasks_waiting: FxHashSet<Vector2<i32>>,
    entity_ticking: SquareUnion,
    block_ticking: SquareUnion,
    spawn_candidates: SquareUnion,
    forced_chunks: FxHashSet<Vector2<i32>>,
}

/// The squares one ticket source covers: entity ticking, block ticking and spawn candidates.
type Squares = [Option<Square>; 3];

impl ActivePlayerArea {
    /// Vanilla's `PLAYER_SIMULATION` ticket sits at `ENTITY_TICKING - simulationDistance`, so
    /// chunks entity tick out to the simulation distance and block tick one ring further.
    const fn squares(self) -> Squares {
        let center = self.center;
        let radius = self.simulation_distance;
        [
            Some(Square { center, radius }),
            Some(Square {
                center,
                radius: radius + 1,
            }),
            Some(Square {
                center,
                radius: NATURAL_SPAWN_CHUNK_RANGE,
            }),
        ]
    }
}

/// Vanilla's `FORCED` ticket sits at the entity ticking level: the chunk itself entity ticks,
/// its first ring block ticks, and it doesn't make spawn candidates.
const fn forced_squares(center: Vector2<i32>) -> Squares {
    [
        Some(Square { center, radius: 0 }),
        Some(Square { center, radius: 1 }),
        None,
    ]
}

impl ActiveChunkTracker {
    fn update_squares(
        &mut self,
        from: Squares,
        to: Squares,
        active: &mut ActiveChunks,
        newly_block_ticking: &mut Vec<Vector2<i32>>,
    ) {
        self.entity_ticking
            .update(from[0], to[0], &mut active.entity_ticking, |_| {}, |_| {});
        let loaded = &mut self.loaded_block_ticking_chunks;
        let ticking = &mut active.ticking;
        self.block_ticking.update(
            from[1],
            to[1],
            &mut active.block_ticking,
            |pos| newly_block_ticking.push(pos),
            |pos| {
                loaded.remove(&pos);
                ticking.remove(&pos);
            },
        );
        self.spawn_candidates
            .update(from[2], to[2], &mut active.spawn_candidates, |_| {}, |_| {});
    }

    pub fn update_player(
        &mut self,
        id: Uuid,
        area: ActivePlayerArea,
        active: &mut ActiveChunks,
        newly_block_ticking: &mut Vec<Vector2<i32>>,
    ) {
        let previous = self.players.insert(id, area);
        if previous == Some(area) {
            return;
        }
        let from = previous.map_or([None; 3], ActivePlayerArea::squares);
        self.update_squares(from, area.squares(), active, newly_block_ticking);
    }

    pub fn remove_player(&mut self, id: Uuid, active: &mut ActiveChunks) {
        if let Some(area) = self.players.remove(&id) {
            self.update_squares(area.squares(), [None; 3], active, &mut Vec::new());
        }
    }

    /// Applies a new forced chunk set and returns the chunks that were added and removed.
    pub fn sync_forced_chunks(
        &mut self,
        forced_chunks: &FxHashSet<Vector2<i32>>,
        active: &mut ActiveChunks,
        newly_block_ticking: &mut Vec<Vector2<i32>>,
    ) -> (Vec<Vector2<i32>>, Vec<Vector2<i32>>) {
        let removed: Vec<_> = self
            .forced_chunks
            .difference(forced_chunks)
            .copied()
            .collect();
        let added: Vec<_> = forced_chunks
            .difference(&self.forced_chunks)
            .copied()
            .collect();
        for &pos in &removed {
            self.update_squares(forced_squares(pos), [None; 3], active, newly_block_ticking);
        }
        for &pos in &added {
            self.update_squares([None; 3], forced_squares(pos), active, newly_block_ticking);
        }
        self.forced_chunks.clone_from(forced_chunks);
        (added, removed)
    }

    /// Queues the waiting chunks whose neighbours have loaded since, and forgets the ones that
    /// stopped block ticking.
    pub fn requeue_waiting_ticking_tasks(
        &mut self,
        neighbours_loaded: impl Fn(&Vector2<i32>) -> bool,
    ) {
        let loaded = &self.loaded_block_ticking_chunks;
        let queue = &mut self.ticking_tasks;
        self.ticking_tasks_waiting.retain(|pos| {
            if !loaded.contains(pos) {
                return false;
            }
            if neighbours_loaded(pos) {
                queue.push_back(*pos);
                return false;
            }
            true
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(x: i32, z: i32, simulation_distance: i32) -> ActivePlayerArea {
        ActivePlayerArea {
            center: Vector2::new(x, z),
            simulation_distance,
        }
    }

    #[test]
    fn follows_player_boundary_crossing() {
        let id = Uuid::from_u128(1);
        let mut tracker = ActiveChunkTracker::default();
        let mut active = ActiveChunks::default();
        let mut newly_block_ticking = Vec::new();

        tracker.update_player(id, area(0, 0, 1), &mut active, &mut newly_block_ticking);
        assert_eq!(active.entity_ticking.len(), 9);
        assert_eq!(active.block_ticking.len(), 25);
        newly_block_ticking.clear();

        tracker.update_player(id, area(1, 0, 1), &mut active, &mut newly_block_ticking);

        assert_eq!(active.entity_ticking.len(), 9);
        assert_eq!(active.block_ticking.len(), 25);
        assert_eq!(newly_block_ticking.len(), 5);
        assert!(!active.entity_ticking.contains(&Vector2::new(-1, 0)));
        assert!(active.entity_ticking.contains(&Vector2::new(2, 0)));
        assert!(active.block_ticking.contains(&Vector2::new(-1, 0)));
        assert!(!active.block_ticking.contains(&Vector2::new(-2, 0)));
    }

    #[test]
    fn removes_chunks_when_player_leaves() {
        let id = Uuid::from_u128(1);
        let mut tracker = ActiveChunkTracker::default();
        let mut active = ActiveChunks::default();

        tracker.update_player(id, area(0, 0, 1), &mut active, &mut Vec::new());
        tracker.remove_player(id, &mut active);

        assert!(active.entity_ticking.is_empty());
        assert!(active.block_ticking.is_empty());
        assert!(active.spawn_candidates.is_empty());
        assert!(tracker.entity_ticking.counts.is_empty());
        assert!(tracker.block_ticking.counts.is_empty());
        assert!(tracker.spawn_candidates.counts.is_empty());
    }

    #[test]
    fn keeps_overlaps_active_until_both_players_leave() {
        let first = Uuid::from_u128(1);
        let second = Uuid::from_u128(2);
        let mut tracker = ActiveChunkTracker::default();
        let mut active = ActiveChunks::default();

        tracker.update_player(first, area(0, 0, 1), &mut active, &mut Vec::new());
        tracker.update_player(second, area(0, 0, 1), &mut active, &mut Vec::new());
        tracker.remove_player(first, &mut active);

        assert_eq!(active.entity_ticking.len(), 9);
        assert!(
            tracker
                .entity_ticking
                .counts
                .values()
                .all(|count| *count == 1)
        );

        tracker.remove_player(second, &mut active);
        assert!(active.entity_ticking.is_empty());
    }

    #[test]
    fn forced_chunk_entity_ticks_alone_and_block_ticks_its_ring() {
        let mut tracker = ActiveChunkTracker::default();
        let mut active = ActiveChunks::default();
        let forced = FxHashSet::from_iter([Vector2::new(5, 5)]);

        tracker.sync_forced_chunks(&forced, &mut active, &mut Vec::new());
        assert_eq!(active.entity_ticking.len(), 1);
        assert_eq!(active.block_ticking.len(), 9);
        assert!(active.spawn_candidates.is_empty());

        tracker.sync_forced_chunks(&FxHashSet::default(), &mut active, &mut Vec::new());
        assert!(active.entity_ticking.is_empty());
        assert!(active.block_ticking.is_empty());
    }
}
