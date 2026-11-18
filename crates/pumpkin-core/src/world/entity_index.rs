use std::sync::{Arc, RwLock, Weak};

use arc_swap::ArcSwap;

use pumpkin_util::math::boundingbox::BoundingBox;
use rustc_hash::FxHashMap;
use uuid::Uuid;

use crate::entity::EntityBase;
use crate::entity::mob::has_game_event_listener;
use crate::entity::player::Player;
use crossbeam::atomic::AtomicCell;

type Registry = Vec<Arc<dyn EntityBase>>;
type PlayerRegistry = Vec<Arc<Player>>;
type Cell = [i32; 3];
type Column = [i32; 2];

const CELL_SIZE: f64 = 4.0;
// This limits index allocation, not entity size or query coverage.
const MAX_ENTITY_CELLS: u64 = 64;
// Cells per column side, so columns span 16 blocks and the full height.
const COLUMN_CELLS: i32 = 4;
const COLUMN_QUERY_RATIO: u64 = 8;

#[derive(Clone, Copy, PartialEq, Eq)]
struct CellRange {
    min: Cell,
    max: Cell,
}

impl CellRange {
    fn of(area: &BoundingBox) -> Option<Self> {
        let min = [area.min.x, area.min.y, area.min.z];
        let max = [area.max.x, area.max.y, area.max.z];
        if min
            .iter()
            .zip(max)
            .any(|(min, max)| !min.is_finite() || !max.is_finite() || *min > max)
        {
            return None;
        }
        Some(Self {
            min: min.map(|n| (n / CELL_SIZE).floor() as i32),
            max: max.map(|n| (n / CELL_SIZE).floor() as i32),
        })
    }

    fn count(self) -> u64 {
        self.min
            .into_iter()
            .zip(self.max)
            .fold(1u64, |count, (min, max)| {
                count.saturating_mul((i64::from(max) - i64::from(min) + 1) as u64)
            })
    }

    fn cells(self) -> impl Iterator<Item = Cell> {
        (self.min[0]..=self.max[0]).flat_map(move |x| {
            (self.min[1]..=self.max[1])
                .flat_map(move |y| (self.min[2]..=self.max[2]).map(move |z| [x, y, z]))
        })
    }

    fn contains(self, cell: Cell) -> bool {
        (0..3).all(|axis| self.min[axis] <= cell[axis] && cell[axis] <= self.max[axis])
    }

    const fn column_bounds(self) -> (Column, Column) {
        (
            [
                self.min[0].div_euclid(COLUMN_CELLS),
                self.min[2].div_euclid(COLUMN_CELLS),
            ],
            [
                self.max[0].div_euclid(COLUMN_CELLS),
                self.max[2].div_euclid(COLUMN_CELLS),
            ],
        )
    }

    fn column_count(self) -> u64 {
        let (min, max) = self.column_bounds();
        min.into_iter().zip(max).fold(1u64, |count, (min, max)| {
            count.saturating_mul((i64::from(max) - i64::from(min) + 1) as u64)
        })
    }

    fn columns(self) -> impl Iterator<Item = Column> {
        let (min, max) = self.column_bounds();
        (min[0]..=max[0]).flat_map(move |x| (min[1]..=max[1]).map(move |z| [x, z]))
    }

    fn contains_column(self, column: Column) -> bool {
        let (min, max) = self.column_bounds();
        (0..2).all(|axis| min[axis] <= column[axis] && column[axis] <= max[axis])
    }
}

struct Tracked {
    slot: usize,
    range: Option<CellRange>,
    synced: u32,
}

/// Entity IDs by fine cell, and by column of cells for wide queries.
#[derive(Default)]
struct Buckets {
    cells: FxHashMap<Cell, Vec<i32>>,
    columns: FxHashMap<Column, Vec<i32>>,
    oversized: Vec<i32>,
}

impl Buckets {
    fn insert(&mut self, entity_id: i32, range: Option<CellRange>) {
        let Some(range) = range else {
            self.oversized.push(entity_id);
            return;
        };
        for cell in range.cells() {
            self.cells.entry(cell).or_default().push(entity_id);
        }
        for column in range.columns() {
            self.columns.entry(column).or_default().push(entity_id);
        }
    }

    fn remove(&mut self, entity_id: i32, range: Option<CellRange>) {
        let Some(range) = range else {
            self.oversized.retain(|member| *member != entity_id);
            return;
        };
        for cell in range.cells() {
            remove_member(&mut self.cells, cell, entity_id);
        }
        for column in range.columns() {
            remove_member(&mut self.columns, column, entity_id);
        }
    }
}

fn remove_member<K: std::hash::Hash + Eq>(
    map: &mut FxHashMap<K, Vec<i32>>,
    key: K,
    entity_id: i32,
) {
    if let std::collections::hash_map::Entry::Occupied(mut members) = map.entry(key) {
        members.get_mut().retain(|member| *member != entity_id);
        if members.get().is_empty() {
            members.remove();
        }
    }
}

/// Cells hold entity IDs, so a registry change only moves slots instead of rebuilding them.
#[derive(Default)]
struct Index {
    registry: Weak<Registry>,
    generation: u32,
    entries: FxHashMap<i32, Tracked>,
    uuids: FxHashMap<Uuid, usize>,
    buckets: Buckets,
    /// Slots of mobs with game event listeners, in list order.
    listeners: Vec<usize>,
    /// The synced registry's entities by slot.
    slots: Vec<Synced>,
    /// Two slots share an entity ID or a UUID, so only a full sync keeps the maps right.
    has_duplicates: bool,
}

/// An entity as of the last sync. The weak reference keeps its allocation, so an equal pointer
/// in a later registry is the same entity.
struct Synced {
    entity: Weak<dyn EntityBase>,
    entity_id: i32,
    uuid: Uuid,
    listener: bool,
}

impl Index {
    fn matches(&self, registry: &Arc<Registry>) -> bool {
        self.registry.as_ptr() == Arc::as_ptr(registry)
    }

    fn sync(&mut self, registry: &Arc<Registry>) {
        if self.has_duplicates {
            self.full_sync(registry);
        } else {
            self.sync_changes(registry);
        }
        self.registry = Arc::downgrade(registry);
    }

    /// Syncs only what changed: the synced entities missing from the registry, in its order,
    /// are removed, and the registry's entities after the last one kept are added. Spawning,
    /// removing and re-ordering spawns at the end keep the rest in place. Bounds changes of
    /// kept entities already went through `update`, so this ends up like a full sync.
    fn sync_changes(&mut self, registry: &Arc<Registry>) {
        let mut kept = 0;
        let mut removed = Vec::new();
        for (slot, synced) in self.slots.iter().enumerate() {
            if registry.get(kept).is_some_and(|entity| {
                std::ptr::addr_eq(Arc::as_ptr(entity), synced.entity.as_ptr())
            }) {
                kept += 1;
            } else {
                removed.push(slot);
            }
        }
        if let Some(&first_removed) = removed.first() {
            // Without duplicates, each removed entity owns its entry and UUID.
            for &slot in &removed {
                let synced = &self.slots[slot];
                if let Some(entry) = self.entries.remove(&synced.entity_id) {
                    self.buckets.remove(synced.entity_id, entry.range);
                }
                self.uuids.remove(&synced.uuid);
            }
            let mut removed = removed.into_iter().peekable();
            let mut slot = 0;
            self.slots.retain(|_| {
                let keep = removed.next_if_eq(&slot).is_none();
                slot += 1;
                keep
            });
            for (slot, synced) in self.slots.iter().enumerate().skip(first_removed) {
                if let Some(entry) = self.entries.get_mut(&synced.entity_id) {
                    entry.slot = slot;
                }
                self.uuids.insert(synced.uuid, slot);
            }
            self.listeners = self
                .slots
                .iter()
                .enumerate()
                .filter_map(|(slot, synced)| synced.listener.then_some(slot))
                .collect();
        }
        for (slot, registered) in registry.iter().enumerate().skip(kept) {
            self.sync_slot(slot, registered);
        }
    }

    fn full_sync(&mut self, registry: &Arc<Registry>) {
        self.generation = self.generation.wrapping_add(1);
        self.uuids.clear();
        self.listeners.clear();
        self.slots.clear();
        self.has_duplicates = false;
        for (slot, registered) in registry.iter().enumerate() {
            self.sync_slot(slot, registered);
        }
        let generation = self.generation;
        let buckets = &mut self.buckets;
        self.entries.retain(|&entity_id, entry| {
            let kept = entry.synced == generation;
            if !kept {
                buckets.remove(entity_id, entry.range);
            }
            kept
        });
    }

    /// Indexes the entity at `slot` of the registry being synced, after the slots before it.
    fn sync_slot(&mut self, slot: usize, registered: &Arc<dyn EntityBase>) {
        let listener = registered.get_mob().is_some_and(has_game_event_listener);
        if listener {
            self.listeners.push(slot);
        }
        let entity = registered.get_entity();
        self.slots.push(Synced {
            entity: Arc::downgrade(registered),
            entity_id: entity.entity_id,
            uuid: entity.entity_uuid,
            listener,
        });
        // The first in list order wins, like a linear search.
        match self.uuids.entry(entity.entity_uuid) {
            std::collections::hash_map::Entry::Occupied(_) => self.has_duplicates = true,
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(slot);
            }
        }
        let range = Self::entity_range(&entity.bounding_box.load());
        match self.entries.entry(entity.entity_id) {
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                let entry = entry.get_mut();
                // Every entry is synced in the current generation once a sync is done.
                if entry.synced == self.generation {
                    self.has_duplicates = true;
                }
                entry.slot = slot;
                entry.synced = self.generation;
                if entry.range != range {
                    self.buckets.remove(entity.entity_id, entry.range);
                    self.buckets.insert(entity.entity_id, range);
                    entry.range = range;
                }
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(Tracked {
                    slot,
                    range,
                    synced: self.generation,
                });
                self.buckets.insert(entity.entity_id, range);
            }
        }
    }

    fn entity_range(area: &BoundingBox) -> Option<CellRange> {
        CellRange::of(area).filter(|range| range.count() <= MAX_ENTITY_CELLS)
    }

    fn update(&mut self, entity_id: i32, area: &BoundingBox) {
        let Some(entry) = self.entries.get_mut(&entity_id) else {
            return;
        };
        let next = Self::entity_range(area);
        if entry.range == next {
            return;
        }
        let previous = std::mem::replace(&mut entry.range, next);
        self.buckets.remove(entity_id, previous);
        self.buckets.insert(entity_id, next);
    }

    /// Slots that may overlap `area`, unsorted and possibly repeated.
    fn candidates(&self, area: &BoundingBox, len: usize) -> Vec<usize> {
        let Some(range) = CellRange::of(area) else {
            return (0..len).collect();
        };
        let slot_of = |entity_id: &i32| self.entries.get(entity_id).map(|entry| entry.slot);
        let mut slots: Vec<usize> = self.buckets.oversized.iter().filter_map(slot_of).collect();
        let buckets = &self.buckets;
        let columns = range.column_count();
        // Wide or tall queries visit far fewer columns than cells.
        if range.count() > columns.saturating_mul(COLUMN_QUERY_RATIO) {
            if columns <= buckets.columns.len() as u64 {
                for column in range.columns() {
                    if let Some(members) = buckets.columns.get(&column) {
                        slots.extend(members.iter().filter_map(slot_of));
                    }
                }
            } else {
                for (&column, members) in &buckets.columns {
                    if range.contains_column(column) {
                        slots.extend(members.iter().filter_map(slot_of));
                    }
                }
            }
        } else if range.count() <= buckets.cells.len() as u64 {
            for cell in range.cells() {
                if let Some(members) = buckets.cells.get(&cell) {
                    slots.extend(members.iter().filter_map(slot_of));
                }
            }
        } else {
            for (&cell, members) in &buckets.cells {
                if range.contains(cell) {
                    slots.extend(members.iter().filter_map(slot_of));
                }
            }
        }
        slots
    }
}

#[derive(Default)]
struct PlayerIndex {
    registry: Weak<PlayerRegistry>,
    slots: FxHashMap<i32, usize>,
    uuids: FxHashMap<Uuid, usize>,
    ranges: Vec<Option<CellRange>>,
    bounds: Vec<AtomicCell<BoundingBox>>,
    cells: FxHashMap<Cell, Vec<usize>>,
    oversized: Vec<usize>,
}

impl PlayerIndex {
    fn matches(&self, registry: &Arc<PlayerRegistry>) -> bool {
        self.registry.as_ptr() == Arc::as_ptr(registry)
    }

    fn rebuild(&mut self, registry: &Arc<PlayerRegistry>) {
        self.slots.clear();
        self.uuids.clear();
        self.ranges.clear();
        self.cells.clear();
        self.oversized.clear();
        self.bounds.clear();
        for (slot, player) in registry.iter().enumerate() {
            let entity = player.get_entity();
            self.slots.insert(entity.entity_id, slot);
            self.uuids.entry(player.gameprofile.id).or_insert(slot);
            let bounds = entity.bounding_box.load();
            self.bounds.push(AtomicCell::new(bounds));
            let range = Index::entity_range(&bounds);
            self.ranges.push(range);
            self.insert(slot, range);
        }
        self.registry = Arc::downgrade(registry);
    }

    fn insert(&mut self, slot: usize, range: Option<CellRange>) {
        if let Some(range) = range {
            for cell in range.cells() {
                self.cells.entry(cell).or_default().push(slot);
            }
        } else {
            self.oversized.push(slot);
        }
    }

    fn update(&mut self, entity_id: i32, area: &BoundingBox) {
        let Some(&slot) = self.slots.get(&entity_id) else {
            return;
        };
        self.bounds[slot].store(*area);
        let next = Index::entity_range(area);
        let previous = self.ranges[slot];
        if previous == next {
            return;
        }
        if let Some(previous) = previous {
            for cell in previous.cells() {
                if let std::collections::hash_map::Entry::Occupied(mut entry) =
                    self.cells.entry(cell)
                {
                    entry.get_mut().retain(|member| *member != slot);
                    if entry.get().is_empty() {
                        entry.remove();
                    }
                }
            }
        } else {
            self.oversized.retain(|member| *member != slot);
        }
        self.ranges[slot] = next;
        self.insert(slot, next);
    }

    fn intersects(&self, slot: usize, area: &BoundingBox) -> bool {
        area.intersects(&self.bounds[slot].load())
    }

    fn candidates(&self, area: &BoundingBox) -> Vec<usize> {
        let Some(range) = CellRange::of(area) else {
            return (0..self.bounds.len()).collect();
        };
        let mut slots: Vec<usize> = self.oversized.clone();
        if range.count() <= self.cells.len() as u64 {
            for cell in range.cells() {
                if let Some(members) = self.cells.get(&cell) {
                    slots.extend(members.iter().copied());
                }
            }
        } else {
            for (&cell, members) in &self.cells {
                if range.contains(cell) {
                    slots.extend(members.iter().copied());
                }
            }
        }
        slots.sort_unstable();
        slots.dedup();
        slots
    }
}

/// Live broad phase for the world's non-player entity queries.
#[derive(Default)]
pub struct EntityIndex {
    inner: RwLock<Index>,
    players: RwLock<PlayerIndex>,
}

impl EntityIndex {
    /// Runs `query` on the index and the current entity list, synced to each other.
    ///
    /// The list is loaded under the lock, so the index only ever syncs forward. Syncing back to
    /// an older list a caller held would make every entity after one removed since look moved,
    /// so the next sync would index them all again.
    fn with_index<T>(
        &self,
        entities: &ArcSwap<Registry>,
        query: impl FnOnce(&Index, Arc<Registry>) -> T,
    ) -> T {
        let index = self
            .inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let registry = entities.load_full();
        if index.matches(&registry) {
            return query(&index, registry);
        }
        drop(index);
        let mut index = self
            .inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let registry = entities.load_full();
        if !index.matches(&registry) {
            index.sync(&registry);
        }
        query(&index, registry)
    }

    fn with_players<T>(
        &self,
        registry: &Arc<PlayerRegistry>,
        query: impl FnOnce(&PlayerIndex) -> T,
    ) -> T {
        let index = self
            .players
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if index.matches(registry) {
            return query(&index);
        }
        drop(index);
        let mut index = self
            .players
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !index.matches(registry) {
            index.rebuild(registry);
        }
        query(&index)
    }

    pub fn query(
        &self,
        entities: &ArcSwap<Registry>,
        area: BoundingBox,
    ) -> impl Iterator<Item = Arc<dyn EntityBase>> + use<> {
        self.query_matching(entities, area, |_| true)
    }

    /// Like [`Self::query`], but drops the candidates `keep` rejects before sorting them or
    /// reading their bounds.
    pub fn query_matching<F: Fn(&dyn EntityBase) -> bool>(
        &self,
        entities: &ArcSwap<Registry>,
        area: BoundingBox,
        keep: F,
    ) -> impl Iterator<Item = Arc<dyn EntityBase>> + use<F> {
        let (mut slots, registry) = self.with_index(entities, |index, registry| {
            (index.candidates(&area, registry.len()), registry)
        });
        slots.retain(|&slot| keep(registry[slot].as_ref()));
        // Keep the registry order, including entities spanning several cells.
        slots.sort_unstable();
        slots.dedup();
        slots.into_iter().filter_map(move |slot| {
            let entity = &registry[slot];
            area.intersects(&entity.get_entity().bounding_box.load())
                .then(|| entity.clone())
        })
    }

    pub fn query_players(
        &self,
        registry: Arc<PlayerRegistry>,
        area: BoundingBox,
    ) -> impl Iterator<Item = Arc<Player>> + use<> {
        let slots = self.with_players(&registry, |index| {
            let mut slots = index.candidates(&area);
            slots.retain(|&slot| index.intersects(slot, &area));
            slots
        });
        slots.into_iter().map(move |slot| registry[slot].clone())
    }

    /// Mobs with game event listeners whose body overlaps `area`, in list order.
    pub fn listeners(
        &self,
        entities: &ArcSwap<Registry>,
        area: BoundingBox,
    ) -> impl Iterator<Item = Arc<dyn EntityBase>> + use<> {
        let (slots, registry) = self.with_index(entities, |index, registry| {
            (index.listeners.clone(), registry)
        });
        slots.into_iter().filter_map(move |slot| {
            let entity = &registry[slot];
            area.intersects(&entity.get_entity().bounding_box.load())
                .then(|| entity.clone())
        })
    }

    pub fn by_id(&self, entities: &ArcSwap<Registry>, id: i32) -> Option<Arc<dyn EntityBase>> {
        self.with_index(entities, |index, registry| {
            index
                .entries
                .get(&id)
                .map(|entry| registry[entry.slot].clone())
        })
    }

    /// The first entity with `uuid` in registry order, without a scan.
    pub fn by_uuid(&self, entities: &ArcSwap<Registry>, uuid: Uuid) -> Option<Arc<dyn EntityBase>> {
        self.with_index(entities, |index, registry| {
            index.uuids.get(&uuid).map(|&slot| registry[slot].clone())
        })
    }

    pub fn player_by_id(&self, registry: &Arc<PlayerRegistry>, id: i32) -> Option<Arc<Player>> {
        let slot = self.with_players(registry, |index| index.slots.get(&id).copied())?;
        Some(registry[slot].clone())
    }

    pub fn player_by_uuid(
        &self,
        registry: &Arc<PlayerRegistry>,
        uuid: Uuid,
    ) -> Option<Arc<Player>> {
        let slot = self.with_players(registry, |index| index.uuids.get(&uuid).copied())?;
        Some(registry[slot].clone())
    }

    pub fn set_bounds(&self, entity_id: i32, bounds: &AtomicCell<BoundingBox>, area: BoundingBox) {
        let index = self
            .inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let next = Index::entity_range(&area);
        if index
            .entries
            .get(&entity_id)
            .is_none_or(|entry| entry.range == next)
        {
            bounds.store(area);
            return;
        }
        drop(index);
        let mut index = self
            .inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        index.update(entity_id, &area);
        bounds.store(area);
    }

    pub fn set_player_bounds(
        &self,
        entity_id: i32,
        bounds: &AtomicCell<BoundingBox>,
        area: BoundingBox,
    ) {
        let index = self
            .players
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let next = Index::entity_range(&area);
        match index.slots.get(&entity_id) {
            None => {
                bounds.store(area);
                return;
            }
            Some(&slot) if index.ranges[slot] == next => {
                index.bounds[slot].store(area);
                bounds.store(area);
                return;
            }
            Some(_) => {}
        }
        drop(index);
        let mut index = self
            .players
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        index.update(entity_id, &area);
        bounds.store(area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::{
        Entity, ai::test_support::world, interaction::InteractionEntity, passive::cow::CowEntity,
    };
    use pumpkin_data::entity::EntityType;
    use pumpkin_util::math::vector3::Vector3;

    fn ids(world: &super::super::World, area: &BoundingBox) -> Vec<i32> {
        world
            .get_entities_at_box(area)
            .iter()
            .map(|entity| entity.get_entity().entity_id)
            .collect()
    }

    #[tokio::test]
    async fn queries_follow_moves_resizes_and_registry_replacement()
    -> Result<(), Box<dyn std::error::Error>> {
        let folder = tempfile::tempdir()?;
        let world = world(&folder);
        let body = CowEntity::new(Entity::new(
            world.clone(),
            Vector3::new(-4.7, 64.0, 0.0),
            &EntityType::COW,
        ));
        world.entities.store(Arc::new(vec![body.clone()]));
        let area = BoundingBox::new(
            Vector3::new(-4.2, 64.0, -0.1),
            Vector3::new(-4.1, 65.0, 0.1),
        );
        assert!(ids(&world, &area).is_empty());
        assert!(
            world
                .get_entity_by_uuid(body.get_entity().entity_uuid)
                .is_some()
        );
        // A bound change within the same grid cells still changes exact membership.
        body.get_entity().set_pos(Vector3::new(-4.6, 64.0, 0.0));
        assert_eq!(ids(&world, &area), [body.get_entity().entity_id]);
        body.get_entity().set_pos(Vector3::new(8.0, 64.0, 0.0));
        assert!(ids(&world, &area).is_empty());
        body.get_entity().bounding_box.store(BoundingBox::new(
            Vector3::new(-5.0, 64.0, -13.0),
            Vector3::new(21.0, 65.0, 13.0),
        ));
        assert_eq!(ids(&world, &area), [body.get_entity().entity_id]);
        body.get_entity()
            .set_pose(pumpkin_data::entity::EntityPose::Dying);
        assert!(ids(&world, &area).is_empty());
        let replacement = InteractionEntity::new(Entity::new(
            world.clone(),
            Vector3::new(-4.0, 64.0, 0.0),
            &EntityType::INTERACTION,
        ));
        world.entities.store(Arc::new(vec![replacement.clone()]));
        assert_eq!(ids(&world, &area), [replacement.entity.entity_id]);
        assert!(
            world
                .get_entity_by_id(body.get_entity().entity_id)
                .is_none()
        );
        assert!(
            world
                .get_entity_by_uuid(body.get_entity().entity_uuid)
                .is_none()
        );
        assert!(
            world
                .get_entity_by_uuid(replacement.entity.entity_uuid)
                .is_some()
        );
        world.entities.store(Arc::new(Vec::new()));
        let removed = Arc::downgrade(&replacement);
        assert!(ids(&world, &area).is_empty());
        drop(replacement);
        assert!(removed.upgrade().is_none());
        world.level.shutdown().await;
        Ok(())
    }

    #[tokio::test]
    async fn queries_keep_order_and_coverage_across_cells_and_oversized_bounds()
    -> Result<(), Box<dyn std::error::Error>> {
        let folder = tempfile::tempdir()?;
        let world = world(&folder);
        let near = InteractionEntity::new(Entity::new(
            world.clone(),
            Vector3::new(-0.1, 64.0, 0.0),
            &EntityType::INTERACTION,
        ));
        let wide = InteractionEntity::new(Entity::new(
            world.clone(),
            Vector3::new(100.0, 64.0, 0.0),
            &EntityType::INTERACTION,
        ));
        wide.set_width(220.0);
        world
            .entities
            .store(Arc::new(vec![wide.clone(), near.clone()]));
        let area = BoundingBox::new(Vector3::new(-1.0, 63.0, -1.0), Vector3::new(1.0, 66.0, 1.0));
        assert_eq!(
            ids(&world, &area),
            [wide.entity.entity_id, near.entity.entity_id]
        );
        let huge = BoundingBox::new(
            Vector3::new(-1e20, -1e20, -1e20),
            Vector3::new(1e20, 1e20, 1e20),
        );
        assert_eq!(
            ids(&world, &huge),
            [wide.entity.entity_id, near.entity.entity_id]
        );
        // Wide queries go through columns, which still need the exact height check.
        let wide_area = BoundingBox::new(
            Vector3::new(-40.0, 60.0, -40.0),
            Vector3::new(40.0, 68.0, 40.0),
        );
        assert_eq!(
            ids(&world, &wide_area),
            [wide.entity.entity_id, near.entity.entity_id]
        );
        let above = BoundingBox::new(
            Vector3::new(-40.0, 80.0, -40.0),
            Vector3::new(40.0, 88.0, 40.0),
        );
        assert!(ids(&world, &above).is_empty());
        // Registry order can change independently of bounds, including island spawn ordering.
        world
            .entities
            .store(Arc::new(vec![near.clone(), wide.clone()]));
        assert_eq!(
            ids(&world, &area),
            [near.entity.entity_id, wide.entity.entity_id]
        );
        wide.set_width(1.0);
        assert_eq!(ids(&world, &area), [near.entity.entity_id]);
        let mut found = Vec::new();
        world.extend_entities_in_box_where(&mut found, usize::MAX, area, |_| true);
        assert_eq!(
            found
                .iter()
                .map(|entity| entity.get_entity().entity_id)
                .collect::<Vec<_>>(),
            [near.entity.entity_id]
        );
        world.level.shutdown().await;
        Ok(())
    }

    #[tokio::test]
    async fn appended_and_replaced_slots_stay_in_sync() -> Result<(), Box<dyn std::error::Error>> {
        let folder = tempfile::tempdir()?;
        let world = world(&folder);
        let cow = |x| {
            CowEntity::new(Entity::new(
                world.clone(),
                Vector3::new(x, 64.0, 0.0),
                &EntityType::COW,
            ))
        };
        let (a, b, c) = (cow(0.0), cow(8.0), cow(16.0));
        let id = |cow: &Arc<CowEntity>| cow.get_entity().entity_id;
        let area = BoundingBox::new(
            Vector3::new(-2.0, 63.0, -2.0),
            Vector3::new(18.0, 66.0, 2.0),
        );
        world.entities.store(Arc::new(vec![a.clone()]));
        assert_eq!(ids(&world, &area), [id(&a)]);
        world.entities.store(Arc::new(vec![a.clone(), b.clone()]));
        assert_eq!(ids(&world, &area), [id(&a), id(&b)]);
        assert!(
            world
                .get_entity_by_uuid(b.get_entity().entity_uuid)
                .is_some()
        );
        b.get_entity().set_pos(Vector3::new(40.0, 64.0, 0.0));
        assert_eq!(ids(&world, &area), [id(&a)]);
        // Same length as before, but the first slot now holds another entity.
        world.entities.store(Arc::new(vec![b.clone(), c.clone()]));
        assert_eq!(ids(&world, &area), [id(&c)]);
        assert!(
            world
                .get_entity_by_uuid(a.get_entity().entity_uuid)
                .is_none()
        );
        assert!(world.get_entity_by_id(id(&b)).is_some());
        // Re-ordered, like spawns sorted at the end of a tick.
        world.entities.store(Arc::new(vec![c.clone(), b.clone()]));
        b.get_entity().set_pos(Vector3::new(8.0, 64.0, 0.0));
        assert_eq!(ids(&world, &area), [id(&c), id(&b)]);
        world.level.shutdown().await;
        Ok(())
    }

    #[tokio::test]
    async fn transferred_bounds_update_the_destination_index()
    -> Result<(), Box<dyn std::error::Error>> {
        let source_folder = tempfile::tempdir()?;
        let destination_folder = tempfile::tempdir()?;
        let source = world(&source_folder);
        let destination = world(&destination_folder);
        let body = CowEntity::new(Entity::new(
            source.clone(),
            Vector3::new(1.0, 64.0, 0.0),
            &EntityType::COW,
        ));
        let area = BoundingBox::new(Vector3::new(0.0, 63.0, -1.0), Vector3::new(2.0, 66.0, 1.0));
        source.entities.store(Arc::new(vec![body.clone()]));
        assert_eq!(ids(&source, &area), [body.get_entity().entity_id]);
        source.entities.store(Arc::new(Vec::new()));
        body.get_entity().set_world(destination.clone());
        destination.entities.store(Arc::new(vec![body.clone()]));
        assert_eq!(ids(&destination, &area), [body.get_entity().entity_id]);
        body.get_entity().set_pos(Vector3::new(20.0, 64.0, 0.0));
        assert!(ids(&source, &area).is_empty());
        assert!(ids(&destination, &area).is_empty());
        assert_eq!(
            ids(&destination, &area.expand(20.0, 0.0, 0.0)),
            [body.get_entity().entity_id]
        );
        source.level.shutdown().await;
        destination.level.shutdown().await;
        Ok(())
    }
}
