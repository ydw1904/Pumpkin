use std::collections::{HashMap, HashSet};

use pumpkin_data::block_properties::{BedPart, WhiteBedLikeProperties};
use pumpkin_data::tag::Taggable;
use pumpkin_data::{Block, BlockStateId, villager::VillagerProfession};
use pumpkin_util::math::{position::BlockPos, vector2::Vector2, vector3::Vector3};
use uuid::Uuid;

use crate::world::World;

/// Vanilla `PoiTypes.MEETING`, used to place wandering traders near a village bell.
pub const POI_TYPE_MEETING: &str = "minecraft:meeting";
/// Vanilla `PoiTypes.HOME`.
pub const POI_TYPE_HOME: &str = "minecraft:home";

/// The level vanilla's POI distance tracker gives sections with no village nearby.
const MAX_SECTIONS_TO_VILLAGE: i32 = 7;

/// The vanilla POI types in `#minecraft:village`, the only ones villagers claim.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VillagePoiType {
    JobSite(VillagerProfession),
    Home,
    Meeting,
}

impl VillagePoiType {
    /// Vanilla `PoiTypes.forState`, limited to village POIs. Only the head of a bed is a home.
    #[must_use]
    pub fn for_state(state_id: BlockStateId) -> Option<Self> {
        let block = state_id.to_block();
        if block == &Block::BELL {
            return Some(Self::Meeting);
        }
        if block.has_tag(&pumpkin_data::tag::Block::MINECRAFT_BEDS) {
            return (WhiteBedLikeProperties::from_state_id(state_id).part == BedPart::Head)
                .then_some(Self::Home);
        }
        profession_for_block(block).map(Self::JobSite)
    }

    /// The village POI type registered as `key`.
    fn from_key(key: &str) -> Option<Self> {
        (0..)
            .map_while(VillagerProfession::from_i32)
            .map(Self::JobSite)
            .chain([Self::Home, Self::Meeting])
            .find(|poi_type| poi_type.key() == key && key != "minecraft:none")
    }

    /// Vanilla `PoiType.maxTickets`.
    #[must_use]
    pub const fn max_tickets(self) -> usize {
        match self {
            Self::Meeting => 32,
            Self::JobSite(_) | Self::Home => 1,
        }
    }

    /// Vanilla `PoiType.validRange`.
    #[must_use]
    pub const fn valid_range(self) -> i32 {
        match self {
            Self::Meeting => 6,
            Self::JobSite(_) | Self::Home => 1,
        }
    }

    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Home => POI_TYPE_HOME,
            Self::Meeting => POI_TYPE_MEETING,
            Self::JobSite(profession) => match profession {
                VillagerProfession::Armorer => "minecraft:armorer",
                VillagerProfession::Butcher => "minecraft:butcher",
                VillagerProfession::Cartographer => "minecraft:cartographer",
                VillagerProfession::Cleric => "minecraft:cleric",
                VillagerProfession::Farmer => "minecraft:farmer",
                VillagerProfession::Fisherman => "minecraft:fisherman",
                VillagerProfession::Fletcher => "minecraft:fletcher",
                VillagerProfession::Leatherworker => "minecraft:leatherworker",
                VillagerProfession::Librarian => "minecraft:librarian",
                VillagerProfession::Mason => "minecraft:mason",
                VillagerProfession::Shepherd => "minecraft:shepherd",
                VillagerProfession::Toolsmith => "minecraft:toolsmith",
                VillagerProfession::Weaponsmith => "minecraft:weaponsmith",
                // `profession_for_block` never yields these.
                VillagerProfession::None | VillagerProfession::Nitwit => "minecraft:none",
            },
        }
    }
}

/// Vanilla `PoiManager.Occupancy`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Occupancy {
    HasSpace,
    IsOccupied,
    Any,
}

/// Vanilla `PoiRecord`.
struct PoiRecord {
    poi_type: VillagePoiType,
    free_tickets: i32,
}

impl PoiRecord {
    const fn new(poi_type: VillagePoiType) -> Self {
        Self {
            poi_type,
            free_tickets: poi_type.max_tickets() as i32,
        }
    }

    /// Vanilla `PoiRecord.acquireTicket`.
    const fn acquire_ticket(&mut self) -> bool {
        if self.free_tickets <= 0 {
            return false;
        }
        self.free_tickets -= 1;
        true
    }

    /// Vanilla `PoiRecord.releaseTicket`.
    const fn release_ticket(&mut self) -> bool {
        if self.free_tickets >= self.poi_type.max_tickets() as i32 {
            return false;
        }
        self.free_tickets += 1;
        true
    }

    /// Vanilla `PoiRecord.hasSpace`.
    const fn has_space(&self) -> bool {
        self.free_tickets > 0
    }

    /// Vanilla `PoiRecord.isOccupied`.
    const fn is_occupied(&self) -> bool {
        self.free_tickets != self.poi_type.max_tickets() as i32
    }

    const fn matches(&self, occupancy: Occupancy) -> bool {
        match occupancy {
            Occupancy::HasSpace => self.has_space(),
            Occupancy::IsOccupied => self.is_occupied(),
            Occupancy::Any => true,
        }
    }
}

/// The village part of vanilla's `PoiManager`. Free tickets are saved with the other POI data,
/// so claims outlive restarts like in vanilla.
#[derive(Default)]
pub struct VillagerPoiStorage {
    records: HashMap<BlockPos, PoiRecord>,
    /// Records whose free tickets changed since the last save.
    dirty_tickets: HashSet<BlockPos>,
    distance_tracker: DistanceTracker,
    recovered: RecoveredClaims,
}

/// Vanilla always saves POI tickets, but older Pumpkin saves didn't keep them reliably. Records
/// read from those saves get their occupancy back from the villagers that remember them, as
/// the villagers load.
#[derive(Default)]
struct RecoveredClaims {
    /// Loaded records from old saves, with the villagers whose claims were counted.
    records: HashMap<BlockPos, HashSet<Uuid>>,
    /// Claims of villagers from old saves on records that aren't loaded yet.
    pending: HashMap<BlockPos, HashSet<Uuid>>,
}

impl VillagerPoiStorage {
    /// Vanilla `ServerLevel.onBlockStateChange`: the record is replaced only when the POI type
    /// changes, so tickets survive state changes like a bed becoming occupied.
    pub fn update_block(&mut self, position: BlockPos, state_id: BlockStateId) {
        let Some(poi_type) = VillagePoiType::for_state(state_id) else {
            if let Some(old) = self.records.remove(&position) {
                self.recovered.records.remove(&position);
                self.distance_tracker
                    .occupancy_changed(position, old.is_occupied(), false);
            }
            return;
        };
        if self
            .records
            .get(&position)
            .is_some_and(|record| record.poi_type == poi_type)
        {
            return;
        }
        if let Some(old) = self.records.insert(position, PoiRecord::new(poi_type)) {
            self.recovered.records.remove(&position);
            self.distance_tracker
                .occupancy_changed(position, old.is_occupied(), false);
        }
    }

    /// Registers a POI of a loaded chunk with the free tickets saved for it. A record still in
    /// memory keeps its own tickets. A record from an old save without reliable tickets starts
    /// with one ticket taken for each villager that already claimed it.
    fn load_record(
        &mut self,
        position: BlockPos,
        poi_type: VillagePoiType,
        free_tickets: i32,
        legacy_format: bool,
    ) {
        if self
            .records
            .get(&position)
            .is_some_and(|record| record.poi_type == poi_type)
        {
            return;
        }
        let max_tickets = poi_type.max_tickets() as i32;
        let claims = self.recovered.pending.remove(&position).unwrap_or_default();
        let free_tickets = if legacy_format {
            self.dirty_tickets.insert(position);
            let free_tickets = max_tickets - claims.len() as i32;
            self.recovered.records.insert(position, claims);
            free_tickets
        } else {
            free_tickets
        };
        let mut record = PoiRecord::new(poi_type);
        record.free_tickets = free_tickets.clamp(0, max_tickets);
        let occupied = record.is_occupied();
        let was_occupied = self
            .records
            .insert(position, record)
            .is_some_and(|old| old.is_occupied());
        self.distance_tracker
            .occupancy_changed(position, was_occupied, occupied);
    }

    /// Vanilla `PoiManager.getType`.
    #[must_use]
    pub fn get_type(&self, position: BlockPos) -> Option<VillagePoiType> {
        self.records.get(&position).map(|record| record.poi_type)
    }

    /// Vanilla `PoiManager.take` limited to the record at `position`: takes a ticket if the
    /// record's type is accepted and one is free.
    pub fn take(
        &mut self,
        position: BlockPos,
        accepts: impl Fn(VillagePoiType) -> bool,
    ) -> Option<VillagePoiType> {
        let record = self.records.get_mut(&position)?;
        let was_occupied = record.is_occupied();
        if !accepts(record.poi_type) || !record.acquire_ticket() {
            return None;
        }
        let poi_type = record.poi_type;
        self.dirty_tickets.insert(position);
        self.distance_tracker
            .occupancy_changed(position, was_occupied, true);
        Some(poi_type)
    }

    /// Takes the ticket a villager loaded from an old save holds on `position`, if the record
    /// came from an old save too and the villager's claim wasn't counted yet.
    pub fn recover_claim(&mut self, position: BlockPos, villager: Uuid) {
        let Some(record) = self.records.get_mut(&position) else {
            self.recovered
                .pending
                .entry(position)
                .or_default()
                .insert(villager);
            return;
        };
        let Some(claims) = self.recovered.records.get_mut(&position) else {
            return;
        };
        let was_occupied = record.is_occupied();
        if claims.insert(villager) && record.acquire_ticket() {
            self.dirty_tickets.insert(position);
            self.distance_tracker
                .occupancy_changed(position, was_occupied, true);
        }
    }

    /// Vanilla `PoiManager.release`.
    pub fn release(&mut self, position: BlockPos) -> bool {
        let Some(record) = self.records.get_mut(&position) else {
            return false;
        };
        let released = record.release_ticket();
        if released {
            let occupied = record.is_occupied();
            self.dirty_tickets.insert(position);
            self.distance_tracker
                .occupancy_changed(position, true, occupied);
        }
        released
    }

    /// The records whose tickets changed since the last call, with their free tickets.
    fn take_dirty_tickets(&mut self) -> Vec<(BlockPos, VillagePoiType, i32)> {
        let dirty = std::mem::take(&mut self.dirty_tickets);
        dirty
            .into_iter()
            .filter_map(|position| {
                let record = self.records.get(&position)?;
                Some((position, record.poi_type, record.free_tickets))
            })
            .collect()
    }

    /// Drops the records of unloaded chunks, returning those whose tickets changed since the
    /// last save. Vanilla keeps every section it ever loaded; Paper unloads them with their
    /// chunks, so long-running servers don't keep the POIs of every chunk ever visited.
    fn unload_chunks(
        &mut self,
        chunks: &HashSet<Vector2<i32>>,
    ) -> Vec<(BlockPos, VillagePoiType, i32)> {
        let mut changed = Vec::new();
        self.records.retain(|position, record| {
            if !chunks.contains(&Vector2::new(position.0.x >> 4, position.0.z >> 4)) {
                return true;
            }
            if self.dirty_tickets.remove(position) {
                changed.push((*position, record.poi_type, record.free_tickets));
            }
            self.recovered.records.remove(position);
            self.distance_tracker
                .occupancy_changed(*position, record.is_occupied(), false);
            false
        });
        changed
    }

    /// Vanilla `PoiManager.getInRange` in its stream order: chunks row by row (z outer, x
    /// inner), then sections bottom to top.
    #[must_use]
    pub fn get_in_range(
        &self,
        center: BlockPos,
        radius: i32,
        accepts: impl Fn(VillagePoiType) -> bool,
        occupancy: Occupancy,
    ) -> Vec<(VillagePoiType, BlockPos)> {
        let radius_squared = i64::from(radius).pow(2);
        let mut found = self
            .records
            .iter()
            .filter(|(position, record)| {
                (position.0.x - center.0.x).abs() <= radius
                    && (position.0.z - center.0.z).abs() <= radius
                    && block_dist_squared(**position, center) <= radius_squared
                    && accepts(record.poi_type)
                    && record.matches(occupancy)
            })
            .map(|(position, record)| (record.poi_type, *position))
            .collect::<Vec<_>>();
        found.sort_unstable_by_key(|(_, position)| {
            (
                position.0.z >> 4,
                position.0.x >> 4,
                position.0.y >> 4,
                position.0.y,
                position.0.z,
                position.0.x,
            )
        });
        found
    }

    /// Vanilla `PoiManager.getInRange` for the `#minecraft:village` POI types, which are all the
    /// types stored here.
    #[must_use]
    pub fn village_pois_in_range(
        &self,
        center: BlockPos,
        radius: i32,
        occupancy: Occupancy,
    ) -> Vec<BlockPos> {
        self.get_in_range(center, radius, |_| true, occupancy)
            .into_iter()
            .map(|(_, position)| position)
            .collect()
    }

    /// Vanilla `PoiManager.getCountInRange`.
    #[must_use]
    pub fn count_in_range(
        &self,
        center: BlockPos,
        radius: i32,
        accepts: impl Fn(VillagePoiType) -> bool,
        occupancy: Occupancy,
    ) -> usize {
        self.get_in_range(center, radius, accepts, occupancy).len()
    }

    /// Vanilla `PoiManager.find`: the first match in stream order, not the closest.
    pub fn find(
        &self,
        accepts: impl Fn(VillagePoiType) -> bool,
        mut filter: impl FnMut(BlockPos) -> bool,
        center: BlockPos,
        radius: i32,
        occupancy: Occupancy,
    ) -> Option<BlockPos> {
        self.get_in_range(center, radius, accepts, occupancy)
            .into_iter()
            .map(|(_, position)| position)
            .find(|position| filter(*position))
    }

    /// Vanilla `PoiManager.findAllClosestFirstWithType`. `filter` sees every candidate before
    /// sorting, like vanilla's stream does.
    pub fn find_all_closest_first(
        &self,
        accepts: impl Fn(VillagePoiType) -> bool,
        mut filter: impl FnMut(BlockPos) -> bool,
        center: BlockPos,
        radius: i32,
        occupancy: Occupancy,
    ) -> Vec<(VillagePoiType, BlockPos)> {
        let mut found = self
            .get_in_range(center, radius, accepts, occupancy)
            .into_iter()
            .filter(|(_, position)| filter(*position))
            .collect::<Vec<_>>();
        found.sort_by_key(|(_, position)| block_dist_squared(*position, center));
        found
    }

    /// Vanilla `PoiManager.sectionsToVillage`: the chebyshev distance in sections to the nearest
    /// section with an occupied village POI, capped at 7.
    #[must_use]
    pub fn sections_to_village(&self, pos: BlockPos) -> i32 {
        self.distance_tracker.level(section_of(pos))
    }

    #[must_use]
    pub fn get_nearest_job_site(&self, origin: BlockPos, radius: i32) -> Option<BlockPos> {
        let radius_squared = i64::from(radius).pow(2);
        let mut closest = None;
        let mut closest_dist = radius_squared;

        for (position, record) in &self.records {
            if !matches!(record.poi_type, VillagePoiType::JobSite(_)) {
                continue;
            }
            let distance_squared = block_dist_squared(*position, origin);
            if distance_squared <= closest_dist {
                closest = Some(*position);
                closest_dist = distance_squared;
            }
        }
        closest
    }
}

/// Vanilla `SectionPos.of`.
const fn section_of(pos: BlockPos) -> Vector3<i32> {
    Vector3::new(pos.0.x >> 4, pos.0.y >> 4, pos.0.z >> 4)
}

fn chebyshev(delta: Vector3<i32>) -> i32 {
    delta.x.abs().max(delta.y.abs()).max(delta.z.abs())
}

/// Vanilla `PoiManager.DistanceTracker`: the level of each section within 6 of a village
/// center, a section holding an occupied village POI. Vanilla propagates levels through
/// neighbouring sections, which settles on the chebyshev distance to the nearest center, so
/// that distance is kept directly and updated when a section starts or stops being a center.
#[derive(Default)]
struct DistanceTracker {
    /// The occupied village POIs in each village center.
    occupied: HashMap<Vector3<i32>, u32>,
    /// Levels below `MAX_SECTIONS_TO_VILLAGE`; every other section has that level.
    levels: HashMap<Vector3<i32>, i32>,
}

impl DistanceTracker {
    const RANGE: i32 = MAX_SECTIONS_TO_VILLAGE - 1;

    fn level(&self, section: Vector3<i32>) -> i32 {
        self.levels
            .get(&section)
            .copied()
            .unwrap_or(MAX_SECTIONS_TO_VILLAGE)
    }

    fn occupancy_changed(&mut self, pos: BlockPos, was_occupied: bool, occupied: bool) {
        if was_occupied == occupied {
            return;
        }
        let section = section_of(pos);
        if occupied {
            let count = self.occupied.entry(section).or_default();
            *count += 1;
            if *count == 1 {
                self.add_center(section);
            }
        } else if let Some(count) = self.occupied.get_mut(&section) {
            *count -= 1;
            if *count == 0 {
                self.occupied.remove(&section);
                self.remove_center(section);
            }
        }
    }

    fn cube(center: Vector3<i32>) -> impl Iterator<Item = (Vector3<i32>, i32)> {
        (-Self::RANGE..=Self::RANGE).flat_map(move |x| {
            (-Self::RANGE..=Self::RANGE).flat_map(move |y| {
                (-Self::RANGE..=Self::RANGE).map(move |z| {
                    let offset = Vector3::new(x, y, z);
                    (center + offset, chebyshev(offset))
                })
            })
        })
    }

    fn add_center(&mut self, center: Vector3<i32>) {
        for (section, distance) in Self::cube(center) {
            self.levels
                .entry(section)
                .and_modify(|level| *level = (*level).min(distance))
                .or_insert(distance);
        }
    }

    /// Clears the levels the center reached and recomputes them from the centers nearby.
    fn remove_center(&mut self, center: Vector3<i32>) {
        for (section, _) in Self::cube(center) {
            self.levels.remove(&section);
        }
        let nearby = self
            .occupied
            .keys()
            .copied()
            .filter(|other| chebyshev(*other - center) <= 2 * Self::RANGE)
            .collect::<Vec<_>>();
        for other in nearby {
            for (section, distance) in Self::cube(other) {
                if chebyshev(section - center) <= Self::RANGE {
                    self.levels
                        .entry(section)
                        .and_modify(|level| *level = (*level).min(distance))
                        .or_insert(distance);
                }
            }
        }
    }
}

/// Vanilla `Vec3i.distSqr` between two block positions.
fn block_dist_squared(a: BlockPos, b: BlockPos) -> i64 {
    let delta = a.0 - b.0;
    i64::from(delta.x).pow(2) + i64::from(delta.y).pow(2) + i64::from(delta.z).pow(2)
}

impl World {
    /// Vanilla `ServerLevel.isCloseToVillage`.
    #[must_use]
    pub fn is_close_to_village(&self, pos: &BlockPos, sections: i32) -> bool {
        self.villager_poi
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .sections_to_village(*pos)
            <= sections
    }

    /// Vanilla `ServerLevel.isVillage`.
    #[must_use]
    pub fn is_village(&self, pos: &BlockPos) -> bool {
        self.is_close_to_village(pos, 1)
    }

    /// Registers the village POI blocks of a freshly loaded chunk, as vanilla's
    /// `PoiManager.checkConsistencyWithBlocks` does for chunks without saved POI data.
    pub(crate) fn register_chunk_pois(&self, chunk_pos: Vector2<i32>) {
        let Some(found) = self.level.read_chunk_sync(&chunk_pos, |chunk| {
            let sections = chunk
                .section
                .block_sections
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut found = Vec::new();
            for (index, section) in sections.iter().enumerate() {
                if !section.contains_matching(|id| VillagePoiType::for_state(id).is_some()) {
                    continue;
                }
                let base_y = chunk.section.min_y + index as i32 * 16;
                for y in 0..16 {
                    for z in 0..16 {
                        for x in 0..16 {
                            let state_id = section.get(x, y, z);
                            if let Some(poi_type) = VillagePoiType::for_state(state_id) {
                                let pos = BlockPos::new(
                                    chunk_pos.x * 16 + x as i32,
                                    base_y + y as i32,
                                    chunk_pos.y * 16 + z as i32,
                                );
                                found.push((pos, poi_type));
                            }
                        }
                    }
                }
            }
            found
        }) else {
            return;
        };
        if found.is_empty() {
            return;
        }

        let found = {
            let mut saved = self
                .portal_poi
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            found
                .into_iter()
                .map(|(pos, poi_type)| {
                    // Vanilla keeps the free tickets saved with the POI section.
                    let (free_tickets, legacy_format) = saved
                        .get_entry(pos)
                        .filter(|entry| entry.poi_type == poi_type.key())
                        .map(|entry| (entry.free_tickets, entry.legacy_format))
                        .unwrap_or_else(|| {
                            let free_tickets = poi_type.max_tickets() as i32;
                            saved.add_with_free_tickets(pos, poi_type.key(), free_tickets);
                            (free_tickets, false)
                        });
                    (pos, poi_type, free_tickets, legacy_format)
                })
                .collect::<Vec<_>>()
        };
        let mut villager_poi = self
            .villager_poi
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for (pos, poi_type, free_tickets, legacy_format) in found {
            villager_poi.load_record(pos, poi_type, free_tickets, legacy_format);
        }
    }

    /// Vanilla `ServerLevel.onBlockStateChange` for village POIs: registers, replaces or
    /// removes the POI at each changed position, and keeps the saved POI data in step.
    pub(crate) fn update_village_pois(
        &self,
        changes: impl IntoIterator<Item = (BlockPos, BlockStateId)>,
    ) {
        let mut villager_poi = self
            .villager_poi
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut saved = None;
        for (position, state_id) in changes {
            // Portal blocks however placed become portal POIs, so travelling links to them.
            if state_id.to_block_id() == pumpkin_data::BlockId::NETHER_PORTAL {
                saved
                    .get_or_insert_with(|| {
                        self.portal_poi
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                    })
                    .add_portal(position);
            }
            let old_poi = villager_poi.get_type(position);
            villager_poi.update_block(position, state_id);
            let new_poi = VillagePoiType::for_state(state_id);
            if old_poi == new_poi {
                continue;
            }
            let saved = saved.get_or_insert_with(|| {
                self.portal_poi
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
            });
            if old_poi.is_some() {
                saved.remove(&position);
            }
            if let Some(poi_type) = new_poi {
                saved.add_with_free_tickets(
                    position,
                    poi_type.key(),
                    poi_type.max_tickets() as i32,
                );
            }
        }
    }

    /// Drops the village POIs of unloaded chunks, keeping their tickets in the saved POI data.
    pub(crate) fn unload_chunk_pois(&self, chunks: &HashSet<Vector2<i32>>) {
        if chunks.is_empty() {
            return;
        }
        let changed = self
            .villager_poi
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .unload_chunks(chunks);
        if changed.is_empty() {
            return;
        }
        let mut saved = self
            .portal_poi
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for (pos, poi_type, free_tickets) in changed {
            saved.add_with_free_tickets(pos, poi_type.key(), free_tickets);
        }
    }

    /// Vanilla `Villager.releasePoi`: frees a ticket of the POI at `pos` if its type is
    /// accepted. Vanilla loads the POI section to do so, so the saved tickets of a POI in an
    /// unloaded chunk are released too.
    pub(crate) fn release_village_poi(
        &self,
        pos: BlockPos,
        accepts: impl Fn(VillagePoiType) -> bool,
    ) {
        let mut villager_poi = self
            .villager_poi
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(poi_type) = villager_poi.get_type(pos) {
            if accepts(poi_type) {
                villager_poi.release(pos);
            }
            return;
        }
        drop(villager_poi);
        if self
            .level
            .is_chunk_loaded(&Vector2::new(pos.0.x >> 4, pos.0.z >> 4))
        {
            return;
        }
        let mut saved = self
            .portal_poi
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some((poi_type, free_tickets)) = saved.get_entry(pos).and_then(|entry| {
            Some((
                VillagePoiType::from_key(&entry.poi_type)?,
                entry.free_tickets,
            ))
        }) else {
            return;
        };
        // Vanilla `PoiRecord.releaseTicket`.
        if accepts(poi_type) && free_tickets < poi_type.max_tickets() as i32 {
            saved.add_with_free_tickets(pos, poi_type.key(), free_tickets + 1);
        }
    }

    /// Writes the free tickets that changed into the saved POI data, before it is saved.
    pub(crate) fn save_village_poi_tickets(&self) {
        let dirty = self
            .villager_poi
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take_dirty_tickets();
        if dirty.is_empty() {
            return;
        }
        let mut saved = self
            .portal_poi
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for (pos, poi_type, free_tickets) in dirty {
            saved.add_with_free_tickets(pos, poi_type.key(), free_tickets);
        }
    }
}

#[must_use]
pub fn profession_for_block(block: &Block) -> Option<VillagerProfession> {
    match block {
        block if block == &Block::COMPOSTER => Some(VillagerProfession::Farmer),
        block if block == &Block::LECTERN => Some(VillagerProfession::Librarian),
        block if block == &Block::BLAST_FURNACE => Some(VillagerProfession::Armorer),
        block if block == &Block::SMOKER => Some(VillagerProfession::Butcher),
        block if block == &Block::CARTOGRAPHY_TABLE => Some(VillagerProfession::Cartographer),
        block if block == &Block::BREWING_STAND => Some(VillagerProfession::Cleric),
        block if block == &Block::BARREL => Some(VillagerProfession::Fisherman),
        block if block == &Block::FLETCHING_TABLE => Some(VillagerProfession::Fletcher),
        block
            if [
                &Block::CAULDRON,
                &Block::WATER_CAULDRON,
                &Block::LAVA_CAULDRON,
                &Block::POWDER_SNOW_CAULDRON,
            ]
            .contains(&block) =>
        {
            Some(VillagerProfession::Leatherworker)
        }
        block if block == &Block::STONECUTTER => Some(VillagerProfession::Mason),
        block if block == &Block::LOOM => Some(VillagerProfession::Shepherd),
        block if block == &Block::SMITHING_TABLE => Some(VillagerProfession::Toolsmith),
        block if block == &Block::GRINDSTONE => Some(VillagerProfession::Weaponsmith),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_villager_workstation_maps_to_its_profession() {
        let workstations = [
            (&Block::COMPOSTER, VillagerProfession::Farmer),
            (&Block::LECTERN, VillagerProfession::Librarian),
            (&Block::BLAST_FURNACE, VillagerProfession::Armorer),
            (&Block::SMOKER, VillagerProfession::Butcher),
            (&Block::CARTOGRAPHY_TABLE, VillagerProfession::Cartographer),
            (&Block::BREWING_STAND, VillagerProfession::Cleric),
            (&Block::BARREL, VillagerProfession::Fisherman),
            (&Block::FLETCHING_TABLE, VillagerProfession::Fletcher),
            (&Block::CAULDRON, VillagerProfession::Leatherworker),
            (&Block::STONECUTTER, VillagerProfession::Mason),
            (&Block::LOOM, VillagerProfession::Shepherd),
            (&Block::SMITHING_TABLE, VillagerProfession::Toolsmith),
            (&Block::GRINDSTONE, VillagerProfession::Weaponsmith),
        ];

        for (block, profession) in workstations {
            assert_eq!(profession_for_block(block), Some(profession));
            assert_eq!(
                VillagePoiType::for_state(block.default_state.id)
                    .unwrap()
                    .key(),
                format!(
                    "minecraft:{}",
                    profession.translation_key().rsplit('.').next().unwrap()
                )
            );
        }
        assert_eq!(profession_for_block(&Block::DIRT), None);
        assert_eq!(
            VillagePoiType::for_state(Block::DIRT.default_state.id),
            None
        );
    }

    #[test]
    fn only_the_bed_head_is_a_home() {
        let mut props = WhiteBedLikeProperties::default(&Block::RED_BED);
        props.part = BedPart::Head;
        assert_eq!(
            VillagePoiType::for_state(props.to_state_id(&Block::RED_BED)),
            Some(VillagePoiType::Home)
        );
        props.part = BedPart::Foot;
        assert_eq!(
            VillagePoiType::for_state(props.to_state_id(&Block::RED_BED)),
            None
        );
    }

    #[test]
    fn taken_tickets_survive_a_save_and_reload() {
        let bed = BlockPos::new(1, 64, 0);
        let bell = BlockPos::new(2, 64, 0);
        let mut bed_props = WhiteBedLikeProperties::default(&Block::RED_BED);
        bed_props.part = BedPart::Head;
        let mut storage = VillagerPoiStorage::default();
        storage.update_block(bed, bed_props.to_state_id(&Block::RED_BED));
        storage.update_block(bell, Block::BELL.default_state.id);
        let any = |_| true;

        assert!(storage.take(bed, any).is_some());
        assert!(storage.take(bed, any).is_none());
        assert!(storage.take(bell, any).is_some());
        assert!(storage.take(bell, any).is_some());

        let folder = tempfile::tempdir().unwrap();
        let mut saved = pumpkin_world::poi::PoiStorage::new(folder.path().join("poi"));
        for (pos, poi_type, free_tickets) in storage.take_dirty_tickets() {
            saved.add_with_free_tickets(pos, poi_type.key(), free_tickets);
        }
        saved.save_all().unwrap();

        let mut saved = pumpkin_world::poi::PoiStorage::new(folder.path().join("poi"));
        let mut reloaded = VillagerPoiStorage::default();
        for (pos, poi_type) in [(bed, VillagePoiType::Home), (bell, VillagePoiType::Meeting)] {
            let entry = saved.get_entry(pos).unwrap();
            assert_eq!(entry.poi_type, poi_type.key());
            assert!(!entry.legacy_format);
            reloaded.load_record(pos, poi_type, entry.free_tickets, false);
        }
        assert!(reloaded.take(bed, any).is_none());
        assert_eq!(
            reloaded.count_in_range(bell, 0, any, Occupancy::IsOccupied),
            1
        );
        assert!(reloaded.release(bed));
        assert!(!reloaded.release(bed));
        // A chunk loading again must not reset the tickets of a record still in memory.
        reloaded.load_record(bed, VillagePoiType::Home, 0, false);
        assert!(reloaded.take(bed, any).is_some());
    }

    #[test]
    fn unloading_a_chunk_drops_its_records_and_village_center() {
        let bell = BlockPos::new(2, 64, 0);
        let other_bell = BlockPos::new(20, 64, 0);
        let any = |_| true;
        let mut storage = VillagerPoiStorage::default();
        storage.update_block(bell, Block::BELL.default_state.id);
        storage.update_block(other_bell, Block::BELL.default_state.id);
        storage.take(bell, any);
        assert_eq!(storage.take_dirty_tickets().len(), 1);
        storage.take(bell, any);

        let changed = storage.unload_chunks(&HashSet::from([Vector2::new(0, 0)]));
        assert_eq!(changed, [(bell, VillagePoiType::Meeting, 30)]);
        assert_eq!(storage.get_type(bell), None);
        assert_eq!(storage.get_type(other_bell), Some(VillagePoiType::Meeting));
        assert_eq!(storage.sections_to_village(bell), MAX_SECTIONS_TO_VILLAGE);
        assert!(storage.take_dirty_tickets().is_empty());

        storage.load_record(bell, VillagePoiType::Meeting, 30, false);
        assert_eq!(storage.sections_to_village(bell), 0);
        assert_eq!(
            storage.count_in_range(bell, 0, any, Occupancy::IsOccupied),
            1
        );
    }

    #[test]
    fn old_saves_recover_tickets_from_the_villagers_that_claimed_them() {
        let bed = BlockPos::new(1, 64, 0);
        let bell = BlockPos::new(2, 64, 0);
        let [first, second, third] = [Uuid::from_u128(1), Uuid::from_u128(2), Uuid::from_u128(3)];
        let any = |_| true;
        let mut storage = VillagerPoiStorage::default();

        // Villagers can load before or after the records they remember.
        storage.recover_claim(bed, first);
        storage.recover_claim(bell, first);
        storage.load_record(bed, VillagePoiType::Home, 1, true);
        storage.load_record(bell, VillagePoiType::Meeting, 32, true);
        storage.recover_claim(bell, second);
        // A villager loading again (its chunk reloaded) holds the same ticket.
        storage.recover_claim(bell, second);

        assert!(storage.take(bed, any).is_none());
        assert_eq!(storage.sections_to_village(bed), 0);
        let mut taken = 0;
        while storage.take(bell, any).is_some() {
            taken += 1;
        }
        assert_eq!(taken, 30);
        // A second claim on a full home takes nothing.
        storage.recover_claim(bed, third);
        assert!(storage.release(bed));
        assert!(!storage.release(bed));

        // Records saved in the current format keep their saved tickets.
        let other_bed = BlockPos::new(40, 64, 0);
        storage.recover_claim(other_bed, third);
        storage.load_record(other_bed, VillagePoiType::Home, 1, false);
        assert!(storage.take(other_bed, any).is_some());
    }

    #[test]
    fn sections_to_village_follows_tickets_taken_and_released() {
        let mut storage = VillagerPoiStorage::default();
        let bells = [
            BlockPos::new(0, 64, 0),
            BlockPos::new(5, 64, 3),
            BlockPos::new(70, 64, 0),
            BlockPos::new(150, 64, 40),
            BlockPos::new(40, 120, -30),
        ];
        for bell in bells {
            storage.update_block(bell, Block::BELL.default_state.id);
        }
        let probes = (-4..=14)
            .flat_map(|x| (2..=9).flat_map(move |y| (-6..=6).map(move |z| (x, y, z))))
            .map(|(x, y, z)| BlockPos::new(x * 16 + 3, y * 16, z * 16 + 7))
            .collect::<Vec<_>>();
        let expected = |occupied: &[BlockPos], probe: BlockPos| {
            occupied
                .iter()
                .map(|bell| chebyshev(section_of(*bell) - section_of(probe)))
                .min()
                .map_or(MAX_SECTIONS_TO_VILLAGE, |distance| {
                    distance.min(MAX_SECTIONS_TO_VILLAGE)
                })
        };
        let check = |storage: &VillagerPoiStorage, occupied: &[BlockPos]| {
            for probe in &probes {
                assert_eq!(
                    storage.sections_to_village(*probe),
                    expected(occupied, *probe),
                    "{probe:?} with {occupied:?}"
                );
            }
        };

        check(&storage, &[]);
        for bell in bells {
            storage.take(bell, |_| true);
        }
        storage.take(bells[0], |_| true);
        check(&storage, &bells);
        // Two tickets in the first section: one release keeps it a village center.
        storage.release(bells[0]);
        storage.release(bells[1]);
        check(&storage, &bells);
        storage.release(bells[0]);
        check(&storage, &bells[2..]);
        storage.update_block(bells[2], Block::AIR.default_state.id);
        check(&storage, &bells[3..]);
    }
}
