use crate::entity::Entity;
use crate::entity::EntityBase;
use crate::entity::mob::equipment::RegionalDifficulty;
use crate::entity::mob::spawn::SpawnGroupData;
use crate::entity::mob::zombie::ZombieEntityBase;
use crate::entity::mob::{Mob, MobEntity};
use crate::world::World;
use pumpkin_data::BlockState;
use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::world::WorldEvent;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::position::BlockPos;
use std::sync::Arc;

pub struct ZombieEntity {
    entity: Arc<ZombieEntityBase>,
}

impl ZombieEntity {
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

impl ZombieEntity {
    pub(super) fn base(&self) -> &ZombieEntityBase {
        &self.entity
    }
}

impl Mob for ZombieEntity {
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
        self.play_entity_sound(Sound::EntityZombieStep, 0.15, 1.0);
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.entity.mob_entity
    }

    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        _reason: crate::entity::mob::spawn::EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        self.entity.finalize_zombie_spawn(self, world, group_data)
    }

    fn spawn_as_baby(&self) -> bool {
        self.entity.spawn_as_baby()
    }

    fn populate_default_equipment_slots(
        &self,
        world: &Arc<World>,
        difficulty: &RegionalDifficulty,
    ) {
        self.entity
            .populate_default_equipment_slots(world, difficulty);
    }

    fn populate_default_equipment_enchantments(&self, difficulty: &RegionalDifficulty) {
        self.entity
            .populate_default_equipment_enchantments(difficulty);
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

    fn post_tick(&self) {
        self.entity
            .tick_under_water_conversion(&EntityType::DROWNED, WorldEvent::SoundZombieToDrowned);
    }

    fn on_damage_with_context(
        &self,
        _damage_type: DamageType,
        source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> bool {
        self.entity.call_reinforcements(cause.or(source));
        true
    }

    fn hurt_reach(&self) -> Option<crate::world::entity_islands::HurtReach> {
        self.entity.reinforcement_hurt_reach()
    }

    // Converting spawns another type.
    fn has_audited_tick(&self) -> bool {
        !self.entity.converts_next_tick()
    }

    // Hits in other entities' turns join them to this reach, so it covers those calls too.
    fn audited_spawn_distance(&self) -> f64 {
        self.entity.reinforcement_reach()
    }

    fn additional_tick_movement(&self) -> f64 {
        self.entity.reinforcement_reach()
    }
}

impl ZombieEntity {
    #[must_use]
    pub fn can_break_doors(&self) -> bool {
        self.entity.can_break_doors()
    }

    pub fn set_can_break_doors(&self, can_break: bool) {
        self.entity.set_can_break_doors(can_break, self);
    }

    #[must_use]
    pub fn is_baby(&self) -> bool {
        self.entity.is_baby()
    }

    pub fn set_baby(&self, baby: bool) {
        self.entity.set_baby(baby);
    }
}
