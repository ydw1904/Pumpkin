use pumpkin_util::random::gameplay::game_random;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use pumpkin_data::Block;
use pumpkin_data::BlockState;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::{EntityStatus, EntityType};
use pumpkin_protocol::bedrock::server::actor_event::ActorEventID;

use crate::enchantment::EnchantmentHelper;
use pumpkin_data::environment_attribute::Activity;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

use crate::entity::ai::brain::memory::{PackedMemories, types};
use crate::entity::ai::brain::pending::PendingBrainUpdates;
use crate::entity::ai::brain::{Brain, BrainState};
use crate::entity::mob::spawn::SpawnGroupData;
use crate::entity::passive::animal::Animal;
use crate::entity::player::Player;
use crate::entity::r#type::CreatureSpawnLevel;
use crate::entity::{
    Entity, EntityBase,
    ageable::{AgeableData, AgeableMob},
    living::LivingEntity,
    mob::{Mob, MobEntity},
};
use crate::plugin::api::events::entity::entity_target::TargetReason;
use crate::world::World;
use crossbeam::atomic::AtomicCell;

mod ai;

pub use ai::HoglinSpecificSensor;

pub struct HoglinEntity {
    pub mob_entity: MobEntity,
    pub immune_to_zombification: AtomicBool,
    pub time_in_overworld: AtomicI32,
    pub cannot_be_hunted: AtomicBool,
    pub ageable_data: AgeableData,
    /// The `NEAREST_REPELLENT` memory, for `getWalkTargetValue` during the brain tick.
    pub nearest_repellent: AtomicCell<Option<BlockPos>>,
    /// The active non-core activity, for the ambient sound.
    activity: AtomicCell<Option<Activity>>,
    pending_brain_updates: PendingBrainUpdates<Self>,
}

/// Vanilla `HoglinAi.isPosNearNearestRepellent`.
const NEAR_REPELLENT_DISTANCE: i32 = 8;

impl HoglinEntity {
    pub const CONVERSION_TIME: i32 = 300;
    pub const XP_REWARD: u32 = 5;
    pub const BABY_XP_REWARD: u32 = 3;

    pub fn new(entity: Entity) -> Arc<Self> {
        // Vanilla `Hoglin.defineSynchedData`.
        entity.synched_data.define(
            pumpkin_data::tracked_data::hoglin::DATA_IMMUNE_TO_ZOMBIFICATION,
            false,
        );
        let mob_entity = MobEntity::new(entity);
        {
            let mut attributes = mob_entity
                .living_entity
                .attributes
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(health) = attributes.get_mut(&Attributes::MAX_HEALTH.id) {
                health.base_value = 40.0;
                health.dirty.store(true, Ordering::Relaxed);
            }
            if let Some(speed) = attributes.get_mut(&Attributes::MOVEMENT_SPEED.id) {
                speed.base_value = 0.3;
                speed.dirty.store(true, Ordering::Relaxed);
            }
            if let Some(knockback_res) = attributes.get_mut(&Attributes::KNOCKBACK_RESISTANCE.id) {
                knockback_res.base_value = 0.6;
                knockback_res.dirty.store(true, Ordering::Relaxed);
            }
            if let Some(attack_kb) = attributes.get_mut(&Attributes::ATTACK_KNOCKBACK.id) {
                attack_kb.base_value = 1.0;
                attack_kb.dirty.store(true, Ordering::Relaxed);
            }
            if let Some(damage) = attributes.get_mut(&Attributes::ATTACK_DAMAGE.id) {
                damage.base_value = 6.0;
                damage.dirty.store(true, Ordering::Relaxed);
            }
        }
        mob_entity.living_entity.health.store(40.0);

        let hoglin = Self {
            mob_entity,
            immune_to_zombification: AtomicBool::new(false),
            time_in_overworld: AtomicI32::new(0),
            cannot_be_hunted: AtomicBool::new(false),
            ageable_data: AgeableData::default(),
            nearest_repellent: AtomicCell::new(None),
            activity: AtomicCell::new(None),
            pending_brain_updates: PendingBrainUpdates::default(),
        };
        Arc::new(hoglin)
    }

    /// Vanilla `Hoglin.canBeHunted`.
    pub fn can_be_hunted(&self) -> bool {
        !self.is_baby() && !self.cannot_be_hunted.load(Ordering::Relaxed)
    }

    /// Vanilla `HoglinAi.isPacified`. A brain busy in its own tick reads as not pacified.
    pub fn is_pacified(&self) -> bool {
        self.mob_entity.brain_has_memory_value(types::PACIFIED.id())
    }

    /// Runs `update` on the brain now, or right after the brain tick that holds it.
    fn update_brain(&self, update: impl FnOnce(&Self, &mut BrainState) + Send + 'static) {
        self.pending_brain_updates.update(self, update);
    }

    fn is_pos_near_nearest_repellent(&self, pos: &BlockPos) -> bool {
        self.nearest_repellent.load().is_some_and(|repellent| {
            let (dx, dy, dz) = (
                repellent.0.x - pos.0.x,
                repellent.0.y - pos.0.y,
                repellent.0.z - pos.0.z,
            );
            dx * dx + dy * dy + dz * dz < NEAR_REPELLENT_DISTANCE * NEAR_REPELLENT_DISTANCE
        })
    }

    pub fn is_immune_to_zombification(&self) -> bool {
        self.immune_to_zombification.load(Ordering::Relaxed)
    }

    pub fn set_immune_to_zombification(&self, immune: bool) {
        self.immune_to_zombification
            .store(immune, Ordering::Relaxed);
        self.mob_entity.living_entity.entity.set_synced_data(
            pumpkin_data::tracked_data::hoglin::DATA_IMMUNE_TO_ZOMBIFICATION,
            immune,
        );
    }

    pub fn is_converting(&self, world: &World) -> bool {
        !self.is_immune_to_zombification()
            && !self.mob_entity.is_no_ai()
            && world.dimension.piglins_zombify
    }

    fn convert_to_zoglin(&self) {
        let entity = &self.mob_entity.living_entity.entity;
        let world = entity.world.load();
        let pos = entity.pos.load();

        if world.level_info.load().difficulty != pumpkin_util::Difficulty::Peaceful {
            world.play_sound(
                Sound::EntityHoglinConvertedToZombified,
                SoundCategory::Hostile,
                &pos,
            );
        }

        let zoglin = crate::entity::r#type::from_type(
            &EntityType::ZOGLIN,
            pos,
            &world,
            uuid::Uuid::new_v4(),
        );

        let zoglin_base = zoglin.get_entity();
        zoglin_base.set_rotation(entity.yaw.load(), entity.pitch.load());
        zoglin_base.head_yaw.store(entity.head_yaw.load());
        zoglin_base.velocity.store(entity.velocity.load());

        if let Some(living) = zoglin.get_living_entity() {
            living.set_health(self.mob_entity.living_entity.health.load());
        }

        if let Some(custom_name) = &**entity.custom_name.load() {
            zoglin_base.set_custom_name(custom_name.clone());
        }

        // Vanilla ConversionType.SINGLE keeps the baby state.
        if self.is_baby()
            && let Some(mob) = zoglin.get_mob()
        {
            mob.spawn_as_baby();
        }

        world.spawn_entity(zoglin);
        entity.remove();
    }
}

impl AgeableMob for HoglinEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }

    fn age_boundary_reached(&self) {
        let damage = if self.is_baby() { 0.5 } else { 6.0 };
        self.mob_entity
            .living_entity
            .set_attribute_base(&Attributes::ATTACK_DAMAGE, damage);
    }
}

impl Animal for HoglinEntity {
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack.item.has_tag(&tag::Item::MINECRAFT_HOGLIN_FOOD)
    }

    /// Vanilla `Hoglin.canFallInLove`.
    fn can_fall_in_love(&self) -> bool {
        !self.is_pacified() && !self.mob_entity.is_in_love()
    }

    fn get_breed_offspring(&self, _partner: &dyn EntityBase) -> Arc<dyn EntityBase> {
        let entity = self.get_entity();
        let offspring = crate::entity::r#type::from_type(
            entity.entity_type,
            entity.pos.load(),
            &entity.world.load(),
            uuid::Uuid::new_v4(),
        );
        if let Some(mob) = offspring.get_mob() {
            mob.get_mob_entity()
                .persistence_required
                .store(true, Ordering::Relaxed);
        }
        offspring
    }
}

impl Mob for HoglinEntity {
    fn get_ambient_sound_interval(&self) -> i32 {
        crate::entity::passive::animal::AMBIENT_SOUND_INTERVAL
    }

    /// Vanilla `HoglinAi.getSoundForCurrentActivity`.
    fn get_ambient_sound(&self) -> Option<Sound> {
        self.activity.load().map(|activity| {
            ai::sound_for_activity(self, activity, self.nearest_repellent.load().is_some())
        })
    }

    fn make_brain(&self, packed: &PackedMemories) -> Brain {
        ai::make_brain(self, packed)
    }

    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        let entity = self.get_entity();
        let world = entity.world.load_full();
        let time = world.get_world_age();
        let (sound, target) = self.mob_entity.with_brain(self, |brain| {
            brain.tick(&world, self, time);
            self.pending_brain_updates.run(self, brain);
            let sound = ai::update_activity(self, brain);
            self.activity.store(brain.get_active_non_core_activity());
            (sound, brain.get(types::ATTACK_TARGET).cloned())
        });
        if let Some(sound) = sound {
            self.make_sound(sound);
        }
        // `getTarget` reads the brain's attack target.
        let current = self.mob_entity.get_target();
        if current.as_ref().map(|t| t.get_entity().entity_id)
            != target.as_ref().map(|t| t.get_entity().entity_id)
        {
            let reason = if target.is_some() {
                TargetReason::Angry
            } else {
                TargetReason::ForgotTarget
            };
            if !self.set_mob_target(target.clone(), reason) && target.is_some() {
                self.mob_entity.set_target(None);
            }
        }
    }

    /// Vanilla `Hoglin.hurtServer`: `HoglinAi.wasHurtBy` the causing living entity.
    fn on_damage_with_context(
        &self,
        _damage_type: pumpkin_data::damage::DamageType,
        source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> bool {
        if let Some(attacker) = cause.or(source)
            && attacker.get_living_entity().is_some()
            && let Some(attacker) = self
                .get_entity()
                .world
                .load()
                .get_entity_by_id(attacker.get_entity().entity_id)
        {
            self.update_brain(move |hoglin, brain| ai::was_hurt_by(hoglin, brain, attacker));
        }
        true
    }

    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {
        self.play_entity_sound(Sound::EntityHoglinStep, 0.15, 1.0);
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn as_animal(&self) -> Option<&dyn Animal> {
        Some(self)
    }

    /// Vanilla `Hoglin.mobInteract`: feeding it makes it persistent.
    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        let consumed = self.animal_interact(player, item_stack);
        if consumed {
            self.mob_entity
                .persistence_required
                .store(true, Ordering::Relaxed);
        }
        consumed
    }

    // Vanilla `Hoglin.removeWhenFarAway` overrides `Animal`'s.
    fn remove_when_far_away(&self, _distance_sq: f64) -> bool {
        true
    }

    /// Vanilla `Hoglin.getWalkTargetValue`.
    fn get_walk_target_value(&self, level: &dyn CreatureSpawnLevel, pos: &BlockPos) -> f32 {
        if self.is_pos_near_nearest_repellent(pos) {
            -1.0
        } else if level.block(&pos.down()).id == Block::CRIMSON_NYLIUM.id {
            10.0
        } else {
            0.0
        }
    }

    fn finalize_spawn(
        &self,
        _world: &Arc<World>,
        _reason: crate::entity::mob::spawn::EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        if self.get_random().random::<f32>() < 0.2 {
            self.set_baby(true);
        }
        self.finalize_spawn_super(group_data)
    }

    fn should_drop_experience(&self) -> bool {
        true
    }

    fn get_base_experience_reward(&self) -> u32 {
        if self.is_baby() {
            Self::BABY_XP_REWARD
        } else {
            Self::XP_REWARD
        }
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        if self.is_immune_to_zombification() {
            nbt.put_bool("IsImmuneToZombification", true);
        }
        let time = self.time_in_overworld.load(Ordering::Relaxed);
        if time > 0 {
            nbt.put_int("TimeInOverworld", time);
        }
        if self.cannot_be_hunted.load(Ordering::Relaxed) {
            nbt.put_bool("CannotBeHunted", true);
        }
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        if let Some(immune) = nbt.get_bool("IsImmuneToZombification") {
            self.set_immune_to_zombification(immune);
        }
        if let Some(time) = nbt.get_int("TimeInOverworld") {
            self.time_in_overworld.store(time, Ordering::Relaxed);
        }
        if let Some(cannot_hunt) = nbt.get_bool("CannotBeHunted") {
            self.cannot_be_hunted.store(cannot_hunt, Ordering::Relaxed);
        }
    }

    fn mob_tick(&self, _caller: &dyn EntityBase) {
        let entity = &self.mob_entity.living_entity.entity;
        if !entity.is_alive() {
            return;
        }

        let world = entity.world.load();
        if self.is_converting(&world) {
            let time = self.time_in_overworld.fetch_add(1, Ordering::Relaxed) + 1;
            if time > Self::CONVERSION_TIME {
                self.convert_to_zoglin();
            }
        } else {
            self.time_in_overworld.store(0, Ordering::Relaxed);
        }
    }

    /// Vanilla `Hoglin.doHurtTarget`.
    fn do_hurt_target(&self, target: &dyn EntityBase) -> bool {
        if target.get_living_entity().is_none() {
            return false;
        }
        let entity = self.get_entity();
        let world = entity.world.load();
        world.send_entity_status(entity, EntityStatus::StartAttacking, ActorEventID::None);
        self.make_sound(Sound::EntityHoglinAttack);
        if let Some(target) = world.get_entity_by_id(target.get_entity().entity_id) {
            self.update_brain(move |hoglin, brain| ai::on_hit_target(hoglin, brain, &target));
        }
        hurt_and_throw_target(self, target)
    }

    fn blocked_by_item(&self, defender: &dyn EntityBase, _fully_blocked: bool) {
        if !self.is_baby() {
            throw_target(&self.mob_entity.living_entity, defender);
        }
    }
}

/// Vanilla `HoglinBase.hurtAndThrowTarget`, shared with zoglins: adults hit for half their
/// attack damage plus a random part of it and throw what they hurt.
pub fn hurt_and_throw_target(attacker: &dyn Mob, target: &dyn EntityBase) -> bool {
    let living = &attacker.get_mob_entity().living_entity;
    let attack_damage = living.get_attribute_value(&Attributes::ATTACK_DAMAGE) as f32;
    let adult = !living.is_baby();
    let damage = if adult && attack_damage as i32 > 0 {
        attack_damage / 2.0 + attacker.get_random().random_range(0..attack_damage as i32) as f32
    } else {
        attack_damage
    };
    let hurt = target.damage_with_context(
        target,
        damage,
        DamageType::MOB_ATTACK,
        None,
        Some(attacker),
        Some(attacker),
    );
    if hurt {
        EnchantmentHelper::do_post_attack_effects(
            target,
            DamageType::MOB_ATTACK,
            Some(attacker),
            Some(attacker),
        );
        if adult {
            throw_target(living, target);
        }
    }
    hurt
}

/// Throw impulse of vanilla `HoglinBase.throwTarget`, or `None` if the target resists it fully.
fn throw_impulse(
    attack_knockback: f64,
    knockback_resistance: f64,
    dx: f64,
    dz: f64,
    random_offset: f32,
    horizontal_roll: f32,
    vertical_roll: f32,
) -> Option<Vector3<f64>> {
    let strength = attack_knockback - knockback_resistance;
    if strength <= 0.0 {
        return None;
    }
    let horizontal = strength * f64::from(horizontal_roll * 0.5 + 0.2);
    let dir = Vector3::new(dx, 0.0, dz).normalize() * horizontal;
    // Vanilla passes the offset to Vec3.yRot as radians.
    let (sin, cos) = f64::from(random_offset).sin_cos();
    Some(Vector3::new(
        dir.x * cos + dir.z * sin,
        strength * f64::from(vertical_roll) * 0.5,
        dir.z * cos - dir.x * sin,
    ))
}

/// Vanilla `HoglinBase.throwTarget`, shared with zoglins.
pub fn throw_target(attacker: &LivingEntity, target: &dyn EntityBase) {
    let Some(target_living) = target.get_living_entity() else {
        return;
    };
    let my_pos = attacker.entity.pos.load();
    let target_pos = target_living.entity.pos.load();
    let Some(impulse) = throw_impulse(
        attacker.get_attribute_value(&Attributes::ATTACK_KNOCKBACK),
        target_living.get_attribute_value(&Attributes::KNOCKBACK_RESISTANCE),
        target_pos.x - my_pos.x,
        target_pos.z - my_pos.z,
        (game_random().random_range(0..21) - 10) as f32,
        game_random().random::<f32>(),
        game_random().random::<f32>(),
    ) else {
        return;
    };
    target_living.entity.add_velocity(impulse);
    target_living
        .entity
        .velocity_dirty
        .store(true, Ordering::SeqCst);
}

#[cfg(test)]
mod tests {
    use super::throw_impulse;

    #[test]
    fn fully_resistant_target_gets_no_throw() {
        assert!(throw_impulse(1.0, 1.0, 1.0, 0.0, 0.0, 0.5, 0.5).is_none());
    }

    #[test]
    fn throw_scales_with_resistance() {
        let impulse = throw_impulse(1.0, 0.5, 1.0, 0.0, 0.0, 1.0, 1.0).unwrap();
        assert!((impulse.x - 0.5 * 0.7).abs() < 1e-6);
        assert!((impulse.y - 0.25).abs() < 1e-9);
        assert!(impulse.z.abs() < 1e-9);
    }
}
