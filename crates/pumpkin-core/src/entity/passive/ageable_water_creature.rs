use crate::entity::r#type::CreatureSpawnLevel;
use crate::entity::{EntityBase, ageable::AgeableMob};

use super::{animal, water_animal};

pub trait AgeableWaterCreature: AgeableMob {
    fn water_creature_spawn_obstruction(&self, level: &dyn CreatureSpawnLevel) -> bool {
        crate::entity::mob::is_unobstructed(self, level)
    }

    fn handle_air_supply(&self, pre_tick_air_supply: i32, caller: &dyn EntityBase) {
        water_animal::handle_air_supply(self, pre_tick_air_supply, caller);
    }

    fn water_creature_experience_reward(&self) -> u32 {
        animal::experience_reward(self)
    }
}
