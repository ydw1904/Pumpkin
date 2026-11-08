use pumpkin_data::tag::{self, Taggable};

use super::super::BrainTick;
use super::super::memory::{MemoryModuleId, types};
use super::Sensor;
use crate::entity::living::LivingEntity;

const REQUIRES: [MemoryModuleId; 2] = [
    types::NEAREST_VISIBLE_ADULT.id(),
    types::NEAREST_VISIBLE_LIVING_ENTITIES.id(),
];

/// Vanilla `AdultSensor`: the nearest visible adult of the mob's own type. `AdultSensorAnyType`
/// takes any adult in `followable_friendly_mobs` instead.
pub struct AdultSensor {
    any_type: bool,
}

impl AdultSensor {
    #[must_use]
    pub const fn same_type() -> Self {
        Self { any_type: false }
    }

    #[must_use]
    pub const fn any_type() -> Self {
        Self { any_type: true }
    }
}

impl Sensor for AdultSensor {
    fn requires(&self) -> &'static [MemoryModuleId] {
        &REQUIRES
    }

    fn do_tick(&mut self, tick: &mut BrainTick<'_>) {
        let Some(visible) = tick.state.get(types::NEAREST_VISIBLE_LIVING_ENTITIES) else {
            return;
        };
        let entity_type = tick.mob.get_entity().entity_type;
        let any_type = self.any_type;
        let adult = visible.find_closest(&tick.visibility(), |entity| {
            let followed = if any_type {
                entity
                    .get_entity()
                    .entity_type
                    .has_tag(&tag::EntityType::MINECRAFT_FOLLOWABLE_FRIENDLY_MOBS)
            } else {
                entity.get_entity().entity_type == entity_type
            };
            followed
                && !entity
                    .get_living_entity()
                    .is_some_and(LivingEntity::is_baby)
        });
        tick.state.set_optional(types::NEAREST_VISIBLE_ADULT, adult);
    }
}
