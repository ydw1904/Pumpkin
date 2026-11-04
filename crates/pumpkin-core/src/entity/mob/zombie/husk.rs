use pumpkin_data::BlockState;
use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::potion::Effect;
use pumpkin_data::sound::Sound;
use pumpkin_data::world::WorldEvent;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::random::gameplay::game_random;
use std::sync::Arc;

use crate::entity::mob::equipment::RegionalDifficulty;
use crate::entity::mob::zombie::ZombieEntityBase;
use crate::entity::{
    Entity, EntityBase,
    mob::{
        Mob, MobEntity,
        spawn::{EntitySpawnReason, PendingVehicle, SpawnGroupData, finalize_spawn},
    },
    r#type::from_type,
};
use crate::world::World;
use pumpkin_nbt::compound::NbtCompound;
use uuid::Uuid;

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

impl HuskEntity {
    pub(super) fn base(&self) -> &ZombieEntityBase {
        &self.entity
    }
}
const CAMEL_HUSK_JOCKEY_CHANCE: f32 = 0.1;

impl Mob for HuskEntity {
    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        let group_data = self.entity.finalize_zombie_spawn(self, world, group_data);
        let entity = self.get_entity();
        let pos = entity.pos.load();
        let block_pos = entity.block_pos.load();
        if reason == EntitySpawnReason::Natural
            && world.is_space_empty(EntityType::CAMEL_HUSK.get_spawn_aabb(
                f64::from(block_pos.0.x) + 0.5,
                f64::from(block_pos.0.y),
                f64::from(block_pos.0.z) + 0.5,
            ))
            && game_random().random::<f32>() < CAMEL_HUSK_JOCKEY_CHANCE
        {
            self.get_mob_entity().set_item_slot(
                &EquipmentSlot::MAIN_HAND,
                ItemStack::new(1, &Item::IRON_SPEAR),
            );
            let camel_husk = from_type(&EntityType::CAMEL_HUSK, pos, world, Uuid::new_v4());
            finalize_spawn(&camel_husk, world, reason, None);
            let parched = from_type(&EntityType::PARCHED, pos, world, Uuid::new_v4());
            parched.get_entity().set_rotation(entity.yaw.load(), 0.0);
            finalize_spawn(&parched, world, reason, None);
            if let Some(camel_mob) = camel_husk.get_mob() {
                camel_mob.get_mob_entity().add_pending_rider(parched);
            }
            self.get_mob_entity()
                .set_pending_vehicle(PendingVehicle::New(camel_husk));
        }
        group_data
    }

    fn xp_reward(&self) -> u32 {
        self.entity.xp_reward()
    }

    fn can_hold_item(&self, stack: &ItemStack) -> bool {
        self.entity.can_hold_item(stack)
    }

    fn wants_to_pick_up(&self, stack: &ItemStack) -> bool {
        super::wants_to_pick_up(self, stack)
    }

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
        target.add_effect_with_source(
            Effect {
                effect_type: &StatusEffect::HUNGER,
                duration: 140 * difficulty.effective_difficulty as i32,
                amplifier: 0,
                ambient: false,
                show_particles: true,
                show_icon: true,
                blend: false,
            },
            Some(self),
        );
    }

    fn post_tick(&self) {
        self.entity
            .tick_under_water_conversion(&EntityType::ZOMBIE, WorldEvent::SoundHuskToZombie);
    }

    // Converting spawns another type.
    fn has_audited_tick(&self) -> bool {
        !self.entity.converts_next_tick()
    }
}
