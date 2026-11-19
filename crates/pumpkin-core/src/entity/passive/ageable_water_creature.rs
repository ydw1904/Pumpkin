use crate::entity::r#type::CreatureSpawnLevel;
use pumpkin_data::damage::DamageType;

use crate::entity::{
    EntityBase,
    ageable::AgeableMob,
    breath::{DROWNING_DAMAGE, MAX_AIR},
};

use super::{animal, water_animal};

pub trait AgeableWaterCreature: AgeableMob {
    fn water_creature_spawn_obstruction(&self, level: &dyn CreatureSpawnLevel) -> bool {
        crate::entity::mob::is_unobstructed(self, level)
    }

    fn handle_air_supply(&self, pre_tick_air_supply: i32, caller: &dyn EntityBase) {
        water_animal::handle_air_supply(
            self,
            pre_tick_air_supply,
            caller,
            self.get_entity().is_in_water(),
            MAX_AIR,
            DROWNING_DAMAGE,
            DamageType::DROWN,
        );
    }

    fn water_creature_experience_reward(&self) -> u32 {
        animal::experience_reward(self)
    }
}
