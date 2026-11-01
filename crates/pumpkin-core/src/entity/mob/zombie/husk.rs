use pumpkin_data::BlockState;
use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::entity::EntityType;
use pumpkin_data::potion::Effect;
use pumpkin_data::sound::Sound;
use pumpkin_data::world::WorldEvent;
use pumpkin_util::math::position::BlockPos;
use std::sync::Arc;

use crate::entity::mob::equipment::RegionalDifficulty;
use crate::entity::mob::zombie::ZombieEntityBase;
use crate::entity::{
    Entity, EntityBase,
    mob::{Mob, MobEntity},
};
use pumpkin_nbt::compound::NbtCompound;

pub struct HuskEntity {
    entity: Arc<ZombieEntityBase>,
}

impl HuskEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let entity = ZombieEntityBase::new(entity);
        let zombie = Self { entity };
        Arc::new(zombie)
    }

    #[must_use]
    pub fn with_can_break_doors(entity: Entity, can_break_doors: bool) -> Arc<Self> {
        let entity = ZombieEntityBase::with_can_break_doors(entity, can_break_doors);
        let zombie = Self { entity };
        Arc::new(zombie)
    }
}

impl Mob for HuskEntity {
    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {
        self.play_entity_sound(Sound::EntityHuskStep, 0.15, 1.0);
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.entity.mob_entity
    }

    fn spawn_as_baby(&self) -> bool {
        self.entity.spawn_as_baby()
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.entity.mob_write_nbt(nbt);
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.entity.mob_read_nbt(nbt);
    }

    fn killed_entity(&self, victim: &dyn EntityBase) -> bool {
        self.entity.killed_entity(victim)
    }

    fn get_base_experience_reward(&self) -> u32 {
        self.entity.base_experience_reward()
    }

    // Vanilla `Husk.doHurtTarget`: an empty-handed hit makes the target hungry.
    fn on_attack(&self, target: &dyn EntityBase) {
        let living = &self.entity.mob_entity.living_entity;
        let Some(target) = target.get_living_entity() else {
            return;
        };
        if !living
            .entity_equipment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&EquipmentSlot::MAIN_HAND)
            .is_empty()
        {
            return;
        }
        let world = living.entity.world.load_full();
        let difficulty = RegionalDifficulty::at(&world, living.entity.pos.load());
        target.add_effect(Effect {
            effect_type: &StatusEffect::HUNGER,
            duration: 140 * difficulty.effective_difficulty as i32,
            amplifier: 0,
            ambient: false,
            show_particles: true,
            show_icon: true,
            blend: false,
        });
    }

    fn post_tick(&self) {
        self.entity
            .tick_under_water_conversion(&EntityType::ZOMBIE, WorldEvent::SoundHuskToZombie);
    }
}
