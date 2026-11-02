use std::{collections::HashMap, sync::Weak};

use pumpkin_data::{Block, villager::VillagerProfession};
use pumpkin_util::math::{position::BlockPos, vector2::Vector2, vector3::Vector3};

use crate::entity::EntityBase;
use crate::world::World;

/// Vanilla `PoiTypes.MEETING`, used to place wandering traders near a village bell.
pub const POI_TYPE_MEETING: &str = "minecraft:meeting";

/// The level vanilla's POI distance tracker gives sections with no village nearby.
const MAX_SECTIONS_TO_VILLAGE: i32 = 7;

struct JobSite {
    profession: VillagerProfession,
    owner: Option<Weak<dyn EntityBase>>,
}

#[derive(Default)]
pub struct VillagerPoiStorage {
    job_sites: HashMap<BlockPos, JobSite>,
}

impl VillagerPoiStorage {
    fn live_owner(owner: &Weak<dyn EntityBase>) -> Option<std::sync::Arc<dyn EntityBase>> {
        owner.upgrade().filter(|entity| {
            entity
                .get_living_entity()
                .is_none_or(|living| living.health.load() > 0.0)
        })
    }

    pub fn update_block(&mut self, position: BlockPos, block: &Block) {
        let Some(profession) = profession_for_block(block) else {
            self.job_sites.remove(&position);
            return;
        };
        let site = self.job_sites.entry(position).or_insert(JobSite {
            profession,
            owner: None,
        });
        if site.profession != profession {
            *site = JobSite {
                profession,
                owner: None,
            };
        }
    }

    pub fn claim(
        &mut self,
        position: BlockPos,
        block: &Block,
        owner: Weak<dyn EntityBase>,
        expected_profession: Option<VillagerProfession>,
    ) -> Option<VillagerProfession> {
        self.update_block(position, block);
        let site = self.job_sites.get_mut(&position)?;
        if expected_profession.is_some_and(|profession| profession != site.profession) {
            return None;
        }
        if let Some(current_owner) = site.owner.as_ref().and_then(Self::live_owner)
            && current_owner.get_entity().entity_uuid != owner.upgrade()?.get_entity().entity_uuid
        {
            return None;
        }
        site.owner = Some(owner);
        Some(site.profession)
    }

    pub fn release(&mut self, position: BlockPos, owner: uuid::Uuid) {
        let Some(site) = self.job_sites.get_mut(&position) else {
            return;
        };
        if site
            .owner
            .as_ref()
            .and_then(Weak::upgrade)
            .is_none_or(|current| current.get_entity().entity_uuid == owner)
        {
            site.owner = None;
        }
    }

    #[must_use]
    pub fn available_job_sites(
        &self,
        origin: BlockPos,
        radius: i32,
        expected_profession: Option<VillagerProfession>,
    ) -> Vec<BlockPos> {
        let radius_squared = i64::from(radius).pow(2);
        let mut sites = self
            .job_sites
            .iter()
            .filter(|(_, site)| {
                expected_profession.is_none_or(|profession| profession == site.profession)
                    && site.owner.as_ref().and_then(Self::live_owner).is_none()
            })
            .filter_map(|(position, _)| {
                let delta = position.0 - origin.0;
                let distance_squared = i64::from(delta.x).pow(2)
                    + i64::from(delta.y).pow(2)
                    + i64::from(delta.z).pow(2);
                (distance_squared <= radius_squared).then_some((distance_squared, *position))
            })
            .collect::<Vec<_>>();
        sites.sort_unstable_by_key(|(distance, _)| *distance);
        sites.into_iter().map(|(_, position)| position).collect()
    }

    /// Vanilla `PoiManager.sectionsToVillage`: the chebyshev distance in sections to the nearest
    /// section with an occupied village POI, capped at 7. Only job sites track their occupant.
    #[must_use]
    pub fn sections_to_village(&self, pos: BlockPos) -> i32 {
        let section = Vector3::new(pos.0.x >> 4, pos.0.y >> 4, pos.0.z >> 4);
        self.job_sites
            .iter()
            .filter(|(_, site)| site.owner.as_ref().and_then(Self::live_owner).is_some())
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

        for position in self.job_sites.keys() {
            let delta = position.0 - origin.0;
            let distance_squared =
                i64::from(delta.x).pow(2) + i64::from(delta.y).pow(2) + i64::from(delta.z).pow(2);
            if distance_squared <= closest_dist {
                closest = Some(*position);
                closest_dist = distance_squared;
            }
        }
        closest
    }
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
                if !section.contains_matching(|id| poi_type_for_block(id.to_block()).is_some()) {
                    continue;
                }
                let base_y = chunk.section.min_y + index as i32 * 16;
                for y in 0..16 {
                    for z in 0..16 {
                        for x in 0..16 {
                            let block = section.get(x, y, z).to_block();
                            if let Some(poi_type) = poi_type_for_block(block) {
                                let pos = BlockPos::new(
                                    chunk_pos.x * 16 + x as i32,
                                    base_y + y as i32,
                                    chunk_pos.y * 16 + z as i32,
                                );
                                found.push((pos, block, poi_type));
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
            for (pos, block, _) in &found {
                villager_poi.update_block(*pos, block);
            }
        }
        let mut saved = self
            .portal_poi
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for (pos, _, poi_type) in found {
            // Keep the tickets of POIs saved with the chunk.
            if saved.get_in_square(pos, 0, Some(poi_type)).is_empty() {
                saved.add_with_free_tickets(pos, poi_type, 1);
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

#[must_use]
pub fn poi_type_for_block(block: &Block) -> Option<&'static str> {
    if block == &Block::BELL {
        return Some(POI_TYPE_MEETING);
    }
    Some(match profession_for_block(block)? {
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
        VillagerProfession::None | VillagerProfession::Nitwit => return None,
    })
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
                poi_type_for_block(block).unwrap(),
                format!(
                    "minecraft:{}",
                    profession.translation_key().rsplit('.').next().unwrap()
                )
            );
        }
        assert_eq!(profession_for_block(&Block::DIRT), None);
        assert_eq!(poi_type_for_block(&Block::DIRT), None);
    }

    #[test]
    fn available_job_sites_respect_profession_and_vanilla_search_radius() {
        let origin = BlockPos::new(0, 64, 0);
        let lectern = BlockPos::new(48, 64, 0);
        let too_far = BlockPos::new(49, 64, 0);
        let composter = BlockPos::new(1, 64, 0);
        let mut storage = VillagerPoiStorage::default();

        storage.update_block(lectern, &Block::LECTERN);
        storage.update_block(too_far, &Block::LECTERN);
        storage.update_block(composter, &Block::COMPOSTER);

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
