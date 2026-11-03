use std::sync::Arc;
use std::sync::atomic::Ordering;

use pumpkin_data::entity::{EntityStatus, EntityType};
use pumpkin_data::environment_attribute::Activity;
use pumpkin_protocol::bedrock::server::ActorEventID;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::random::gameplay::game_random;

use super::meet::INTERACT_DIST_SQR;
use super::{BREEDING_FOOD_THRESHOLD, VillagerData, VillagerEntity, villager_type_by_biome};
use crate::entity::ageable::{AgeableMob, BABY_START_AGE};
use crate::entity::ai::pathfinder::path::Path;
use crate::entity::mob::Mob;
use crate::entity::{Entity, EntityBase};
use crate::world::World;
use crate::world::villager_poi::{Occupancy, VillagePoiType};

/// The `minDuration` and `maxDuration` of vanilla `VillagerMakeLove`.
const MAKE_LOVE_DURATION: i64 = 350;
/// Vanilla `VillagerMakeLove.start`'s `275 + random.nextInt(50)`.
const MIN_BIRTH_DELAY: i32 = 275;
const BIRTH_DELAY_SPREAD: i32 = 50;
/// The age vanilla `VillagerMakeLove.breed` gives both parents.
const PARENT_AGE: i32 = 6000;
/// The search radius of vanilla `VillagerMakeLove.takeVacantBed`.
const BED_SEARCH_RADIUS: i32 = 48;

/// A running vanilla `VillagerMakeLove`.
#[derive(Clone, Copy)]
pub(super) struct MakeLove {
    birth_timestamp: i64,
    end_timestamp: i64,
}

pub(super) fn as_villager(entity: &dyn EntityBase) -> Option<&VillagerEntity> {
    entity
        .get_mob()?
        .cast_any()
        .downcast_ref::<VillagerEntity>()
}

impl VillagerEntity {
    /// Vanilla `Villager.canBreed`.
    pub(super) fn can_breed(&self) -> bool {
        self.food_level.load(Ordering::Relaxed) + self.count_food_points_in_inventory()
            >= BREEDING_FOOD_THRESHOLD
            && !self.is_sleeping()
            && self.get_age() == 0
    }

    /// Vanilla `Villager.eatAndDigestFood`.
    fn eat_and_digest_food(&self) {
        self.eat_until_full();
        self.food_level
            .fetch_sub(BREEDING_FOOD_THRESHOLD, Ordering::Relaxed);
    }

    pub(super) fn set_breed_target(&self, target: Option<i32>) {
        *self
            .breed_target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = target;
    }

    /// Vanilla `VillagerMakeLove.isBreedingPossible`: the breed target, if it is a living
    /// villager the villager sees and both can breed.
    fn breed_partner(&self, world: &World) -> Option<Arc<dyn EntityBase>> {
        let id = (*self
            .breed_target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner))?;
        let partner = world.get_entity_by_id(id)?;
        let visible = partner.get_entity().entity_type == &EntityType::VILLAGER
            && partner.get_entity().is_alive()
            && self
                .nearest_living_entities(world)
                .iter()
                .any(|other| other.get_entity().entity_id == id)
            && self.has_line_of_sight(partner.get_entity());
        (visible && self.can_breed() && as_villager(partner.as_ref())?.can_breed())
            .then_some(partner)
    }

    /// The `VillagerMakeLove` gate of vanilla's idle package. The gate starts whenever it can,
    /// and stopping it erases `BREED_TARGET` even when `VillagerMakeLove` couldn't start.
    pub(super) fn tick_make_love(&self, world: &Arc<World>, game_time: i64, activity: Activity) {
        let mut make_love = self
            .make_love
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let partner = self.breed_partner(world);
        let Some(running) = *make_love else {
            if activity != Activity::Idle {
                return;
            }
            let Some(partner) = partner else {
                self.set_breed_target(None);
                return;
            };
            let Some(partner_villager) = as_villager(partner.as_ref()) else {
                return;
            };
            // Vanilla `VillagerMakeLove.start`.
            self.lock_gaze_and_walk_to_each_other(partner_villager);
            for entity in [partner.get_entity(), self.get_entity()] {
                world.send_entity_status(
                    entity,
                    EntityStatus::InLoveHearts,
                    ActorEventID::InLoveHearts,
                );
            }
            let running = MakeLove {
                birth_timestamp: game_time
                    + i64::from(
                        MIN_BIRTH_DELAY + game_random().random_range(0..BIRTH_DELAY_SPREAD),
                    ),
                end_timestamp: game_time + MAKE_LOVE_DURATION,
            };
            *make_love = Some(running);
            drop(make_love);
            self.tick_make_love_running(world, game_time, running, partner_villager);
            return;
        };
        match partner {
            Some(partner)
                if game_time <= running.end_timestamp && game_time <= running.birth_timestamp =>
            {
                drop(make_love);
                if let Some(partner_villager) = as_villager(partner.as_ref()) {
                    self.tick_make_love_running(world, game_time, running, partner_villager);
                }
            }
            _ => {
                *make_love = None;
                self.set_breed_target(None);
            }
        }
    }

    /// Vanilla `VillagerMakeLove.tick`.
    fn tick_make_love_running(
        &self,
        world: &Arc<World>,
        game_time: i64,
        running: MakeLove,
        partner: &Self,
    ) {
        let distance_sq = self
            .get_entity()
            .pos
            .load()
            .squared_distance_to_vec(&partner.get_entity().pos.load());
        if distance_sq > INTERACT_DIST_SQR {
            return;
        }
        self.lock_gaze_and_walk_to_each_other(partner);
        if game_time >= running.birth_timestamp {
            self.eat_and_digest_food();
            partner.eat_and_digest_food();
            self.try_to_give_birth(world, partner);
        } else if game_random().random_range(0..35) == 0 {
            for entity in [partner.get_entity(), self.get_entity()] {
                world.send_entity_status(
                    entity,
                    EntityStatus::LoveHearts,
                    ActorEventID::LoveHearts,
                );
            }
        }
    }

    /// Vanilla `VillagerMakeLove.tryToGiveBirth`.
    fn try_to_give_birth(&self, world: &Arc<World>, partner: &Self) {
        let Some(bed) = self.take_vacant_bed(world) else {
            for entity in [partner.get_entity(), self.get_entity()] {
                world.send_entity_status(
                    entity,
                    EntityStatus::VillagerAngry,
                    ActorEventID::VillagerAngry,
                );
            }
            return;
        };
        let child = self.breed(world, partner);
        // Vanilla `VillagerMakeLove.giveBedToChild`.
        *child
            .home_pos
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(bed);
    }

    /// Vanilla `VillagerMakeLove.takeVacantBed`: takes the ticket of the first free bed, in
    /// `PoiManager.getInRange` order rather than by distance, that it can path to.
    fn take_vacant_bed(&self, world: &World) -> Option<BlockPos> {
        let is_home = |poi_type| poi_type == VillagePoiType::Home;
        let beds = world
            .villager_poi
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_in_range(
                self.get_entity().block_pos.load(),
                BED_SEARCH_RADIUS,
                is_home,
                Occupancy::HasSpace,
            );
        let bed = beds.into_iter().find_map(|(poi_type, pos)| {
            self.mob_entity
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .create_path_to_any(
                    &self.mob_entity.living_entity,
                    &[pos],
                    poi_type.valid_range(),
                )
                .filter(Path::can_reach)
                .map(|_| pos)
        })?;
        world
            .villager_poi
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take(bed, is_home)
            .map(|_| bed)
    }

    /// Vanilla `VillagerMakeLove.breed` with `Villager.getBreedOffspring`.
    fn breed(&self, world: &Arc<World>, partner: &Self) -> Arc<Self> {
        let entity = self.get_entity();
        let biome_roll = game_random().random::<f64>();
        let villager_type = if biome_roll < 0.5 {
            villager_type_by_biome(world.get_biome(&entity.block_pos.load()))
        } else if biome_roll < 0.75 {
            self.villager_data().type_enum()
        } else {
            partner.villager_data().type_enum()
        };
        let child = Self::new(Entity::new(
            world.clone(),
            entity.pos.load(),
            &EntityType::VILLAGER,
        ));
        child.set_villager_data(VillagerData::new(
            villager_type,
            super::VillagerProfession::None,
            1,
        ));
        child.villager_data_finalized.store(true, Ordering::Relaxed);
        self.set_age(PARENT_AGE);
        partner.set_age(PARENT_AGE);
        child.set_age(BABY_START_AGE);
        child.get_entity().set_rotation(0.0, 0.0);
        world.spawn_entity(child.clone());
        world.send_entity_status(
            child.get_entity(),
            EntityStatus::LoveHearts,
            ActorEventID::LoveHearts,
        );
        child
    }

    fn villager_data(&self) -> VillagerData {
        *self
            .villager_data
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}
