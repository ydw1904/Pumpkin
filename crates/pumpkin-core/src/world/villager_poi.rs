use std::{collections::HashMap, sync::Weak};

use pumpkin_data::block_properties::{BedPart, WhiteBedLikeProperties};
use pumpkin_data::tag::Taggable;
use pumpkin_data::{Block, BlockStateId, villager::VillagerProfession};
use pumpkin_util::math::{position::BlockPos, vector2::Vector2, vector3::Vector3};

use crate::entity::EntityBase;
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

/// Vanilla `PoiRecord`. Instead of a free ticket counter it keeps the entities holding a
/// ticket, so a ticket frees itself once its holder dies or is removed.
struct PoiRecord {
    poi_type: VillagePoiType,
    owners: Vec<Weak<dyn EntityBase>>,
}

impl PoiRecord {
    const fn new(poi_type: VillagePoiType) -> Self {
        Self {
            poi_type,
            owners: Vec::new(),
        }
    }

    fn live_owner(owner: &Weak<dyn EntityBase>) -> Option<std::sync::Arc<dyn EntityBase>> {
        owner.upgrade().filter(|entity| {
            !entity.get_entity().is_removed()
                && entity
                    .get_living_entity()
                    .is_none_or(|living| living.health.load() > 0.0)
        })
    }

    fn occupants(&self) -> usize {
        self.owners
            .iter()
            .filter(|owner| Self::live_owner(owner).is_some())
            .count()
    }

    fn is_held_by(&self, owner: uuid::Uuid) -> bool {
        self.owners.iter().any(|current| {
            Self::live_owner(current).is_some_and(|entity| entity.get_entity().entity_uuid == owner)
        })
    }

    fn drop_owner(&mut self, owner: uuid::Uuid) {
        self.owners.retain(|current| {
            Self::live_owner(current).is_some_and(|entity| entity.get_entity().entity_uuid != owner)
        });
    }

    /// Vanilla `PoiRecord.hasSpace`.
    fn has_space(&self) -> bool {
        self.occupants() < self.poi_type.max_tickets()
    }

    /// Vanilla `PoiRecord.isOccupied`.
    fn is_occupied(&self) -> bool {
        self.occupants() > 0
    }

    fn matches(&self, occupancy: Occupancy) -> bool {
        match occupancy {
            Occupancy::HasSpace => self.has_space(),
            Occupancy::IsOccupied => self.is_occupied(),
            Occupancy::Any => true,
        }
    }
}

#[derive(Default)]
pub struct VillagerPoiStorage {
    records: HashMap<BlockPos, PoiRecord>,
}

impl VillagerPoiStorage {
    /// Vanilla `ServerLevel.onBlockStateChange`: the record is replaced only when the POI type
    /// changes, so tickets survive state changes like a bed becoming occupied.
    pub fn update_block(&mut self, position: BlockPos, state_id: BlockStateId) {
        let Some(poi_type) = VillagePoiType::for_state(state_id) else {
            self.records.remove(&position);
            return;
        };
        let record = self
            .records
            .entry(position)
            .or_insert_with(|| PoiRecord::new(poi_type));
        if record.poi_type != poi_type {
            *record = PoiRecord::new(poi_type);
        }
    }

    /// Vanilla `PoiManager.getType`.
    #[must_use]
    pub fn get_type(&self, position: BlockPos) -> Option<VillagePoiType> {
        self.records.get(&position).map(|record| record.poi_type)
    }

    /// Vanilla `PoiRecord.acquireTicket` for the record at `position`, if its type is accepted.
    /// Holding a ticket already counts as success.
    pub fn claim(
        &mut self,
        position: BlockPos,
        owner: Weak<dyn EntityBase>,
        accepts: impl Fn(VillagePoiType) -> bool,
    ) -> Option<VillagePoiType> {
        let record = self.records.get_mut(&position)?;
        if !accepts(record.poi_type) {
            return None;
        }
        let owner_uuid = PoiRecord::live_owner(&owner)?.get_entity().entity_uuid;
        if record.is_held_by(owner_uuid) {
            return Some(record.poi_type);
        }
        record
            .owners
            .retain(|current| PoiRecord::live_owner(current).is_some());
        if !record.has_space() {
            return None;
        }
        record.owners.push(owner);
        Some(record.poi_type)
    }

    /// Vanilla `PoiRecord.releaseTicket` for the ticket `owner` holds.
    pub fn release(&mut self, position: BlockPos, owner: uuid::Uuid) {
        if let Some(record) = self.records.get_mut(&position) {
            record.drop_owner(owner);
        }
    }

    /// Hands the ticket `from` holds to `to`. Vanilla `YieldJobSite` leaves the acquired ticket
    /// in place while another villager takes over the memory.
    pub fn transfer(&mut self, position: BlockPos, from: uuid::Uuid, to: Weak<dyn EntityBase>) {
        let Some(record) = self.records.get_mut(&position) else {
            return;
        };
        let Some(to_uuid) =
            PoiRecord::live_owner(&to).map(|entity| entity.get_entity().entity_uuid)
        else {
            return;
        };
        let held = record.is_held_by(from);
        record.drop_owner(from);
        if held && !record.is_held_by(to_uuid) {
            record.owners.push(to);
        }
    }

    /// Vanilla `PoiManager.getInRange` in its stream order: chunks row by row (z outer, x
    /// inner), then sections bottom to top.
    fn in_range(
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

    /// Vanilla `PoiManager.getCountInRange`.
    #[must_use]
    pub fn count_in_range(
        &self,
        center: BlockPos,
        radius: i32,
        accepts: impl Fn(VillagePoiType) -> bool,
        occupancy: Occupancy,
    ) -> usize {
        self.in_range(center, radius, accepts, occupancy).len()
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
        self.in_range(center, radius, accepts, occupancy)
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
            .in_range(center, radius, accepts, occupancy)
            .into_iter()
            .filter(|(_, position)| filter(*position))
            .collect::<Vec<_>>();
        found.sort_by_key(|(_, position)| block_dist_squared(*position, center));
        found
    }

    #[must_use]
    pub fn available_job_sites(
        &self,
        origin: BlockPos,
        radius: i32,
        expected_profession: Option<VillagerProfession>,
    ) -> Vec<BlockPos> {
        self.find_all_closest_first(
            |poi_type| match poi_type {
                VillagePoiType::JobSite(profession) => {
                    expected_profession.is_none_or(|expected| expected == profession)
                }
                VillagePoiType::Home | VillagePoiType::Meeting => false,
            },
            |_| true,
            origin,
            radius,
            Occupancy::HasSpace,
        )
        .into_iter()
        .map(|(_, position)| position)
        .collect()
    }

    /// Vanilla `PoiManager.sectionsToVillage`: the chebyshev distance in sections to the nearest
    /// section with an occupied village POI, capped at 7.
    #[must_use]
    pub fn sections_to_village(&self, pos: BlockPos) -> i32 {
        let section = Vector3::new(pos.0.x >> 4, pos.0.y >> 4, pos.0.z >> 4);
        self.records
            .iter()
            .filter(|(_, record)| record.is_occupied())
            .map(|(site_pos, _)| {
                let delta =
                    Vector3::new(site_pos.0.x >> 4, site_pos.0.y >> 4, site_pos.0.z >> 4) - section;
                delta.x.abs().max(delta.y.abs()).max(delta.z.abs())
            })
            .min()
            .map_or(MAX_SECTIONS_TO_VILLAGE, |distance| {
                distance.min(MAX_SECTIONS_TO_VILLAGE)
            })
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
                                found.push((pos, state_id, poi_type));
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

        {
            let mut villager_poi = self
                .villager_poi
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for (pos, state_id, _) in &found {
                villager_poi.update_block(*pos, *state_id);
            }
        }
        let mut saved = self
            .portal_poi
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for (pos, _, poi_type) in found {
            // Keep the tickets of POIs saved with the chunk.
            if saved.get_in_square(pos, 0, Some(poi_type.key())).is_empty() {
                saved.add_with_free_tickets(pos, poi_type.key(), poi_type.max_tickets() as i32);
            }
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

    #[tokio::test]
    async fn tickets_follow_vanilla_counts_and_free_up_when_the_holder_is_gone() {
        use crate::entity::Entity;
        use crate::entity::passive::villager::VillagerEntity;
        use pumpkin_data::entity::EntityType;
        use std::sync::Arc;

        let folder = tempfile::tempdir().unwrap();
        let world = crate::entity::ai::test_support::world(&folder);
        let villager = || -> std::sync::Arc<dyn EntityBase> {
            VillagerEntity::new(Entity::new(
                world.clone(),
                Vector3::new(0.5, 64.0, 0.5),
                &EntityType::VILLAGER,
            ))
        };
        let (first, second) = (villager(), villager());
        let bed = BlockPos::new(1, 64, 0);
        let bell = BlockPos::new(2, 64, 0);
        let mut bed_props = WhiteBedLikeProperties::default(&Block::RED_BED);
        bed_props.part = BedPart::Head;
        let mut storage = VillagerPoiStorage::default();
        storage.update_block(bed, bed_props.to_state_id(&Block::RED_BED));
        storage.update_block(bell, Block::BELL.default_state.id);
        let any = |_| true;

        assert!(storage.claim(bed, Arc::downgrade(&first), any).is_some());
        assert!(storage.claim(bed, Arc::downgrade(&second), any).is_none());
        assert!(storage.claim(bell, Arc::downgrade(&first), any).is_some());
        assert!(storage.claim(bell, Arc::downgrade(&second), any).is_some());
        assert_eq!(
            storage.count_in_range(bed, 4, any, Occupancy::IsOccupied),
            2
        );

        first.get_living_entity().unwrap().health.store(0.0);
        assert!(storage.claim(bed, Arc::downgrade(&second), any).is_some());
        storage.release(bed, second.get_entity().entity_uuid);
        assert_eq!(
            storage.count_in_range(bed, 0, |t| t == VillagePoiType::Home, Occupancy::HasSpace),
            1
        );
        world.level.shutdown().await;
    }

    #[test]
    fn available_job_sites_respect_profession_and_vanilla_search_radius() {
        let origin = BlockPos::new(0, 64, 0);
        let lectern = BlockPos::new(48, 64, 0);
        let too_far = BlockPos::new(49, 64, 0);
        let composter = BlockPos::new(1, 64, 0);
        let mut storage = VillagerPoiStorage::default();

        storage.update_block(lectern, Block::LECTERN.default_state.id);
        storage.update_block(too_far, Block::LECTERN.default_state.id);
        storage.update_block(composter, Block::COMPOSTER.default_state.id);

        assert_eq!(
            storage.available_job_sites(origin, 48, Some(VillagerProfession::Librarian)),
            vec![lectern]
        );
        assert_eq!(
            storage.available_job_sites(origin, 48, Some(VillagerProfession::Farmer)),
            vec![composter]
        );
    }
}
