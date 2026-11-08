use pumpkin_util::random::gameplay::game_random;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Weak};

use pumpkin_data::Block;
use pumpkin_data::BlockState;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

use crate::entity::ai::goal::avoid_repellent::{
    AvoidRepellentGoal, find_nearest_block_by_manhattan_distance,
};
use crate::entity::mob::spawn::SpawnGroupData;
use crate::entity::passive::animal::Animal;
use crate::entity::player::Player;
use crate::entity::r#type::CreatureSpawnLevel;
use crate::entity::{
    Entity, EntityBase,
    ageable::{AgeableData, AgeableMob},
    ai::goal::{
        active_target::ActiveTargetGoal, breed::BreedGoal, look_around::RandomLookAroundGoal,
        look_at_entity::LookAtEntityGoal, melee_attack::MeleeAttackGoal, revenge::RevengeGoal,
        swim::SwimGoal, wander_around::WanderAroundGoal,
    },
    living::LivingEntity,
    mob::{Mob, MobEntity},
};
use crate::plugin::api::events::entity::entity_target::TargetReason;
use crate::world::World;
use crossbeam::atomic::AtomicCell;

/// Vanilla `HoglinAi` and `Zoglin` melee cooldowns: `MeleeAttack.create(40)` for adults,
/// `MeleeAttack.create(15)` for babies.
pub(crate) const fn hoglin_attack_interval(baby: bool) -> i32 {
    if baby { 15 } else { 40 }
}

pub struct HoglinEntity {
    pub mob_entity: MobEntity,
    pub immune_to_zombification: AtomicBool,
    pub time_in_overworld: AtomicI32,
    pub cannot_be_hunted: AtomicBool,
    pub ageable_data: AgeableData,
    /// The `NEAREST_REPELLENT` memory.
    pub nearest_repellent: AtomicCell<Option<BlockPos>>,
    /// Ticks left of the `PACIFIED` memory.
    pub pacified_timer: AtomicI32,
}

/// Vanilla `HoglinAi` constants.
const REPELLENT_DETECTION_RANGE_HORIZONTAL: i32 = 8;
const REPELLENT_DETECTION_RANGE_VERTICAL: i32 = 4;
const REPELLENT_PACIFY_TIME: i32 = 200;
/// Vanilla `Sensor` default scan rate, which `HoglinSpecificSensor` keeps.
const SENSOR_SCAN_INTERVAL: i64 = 20;
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
            pacified_timer: AtomicI32::new(0),
        };
        let mob_arc = Arc::new(hoglin);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        {
            let goal_selector = &mob_arc.mob_entity.goals_selector;

            goal_selector.add_goal(0, Box::new(SwimGoal::default()));
            // Vanilla `AnimalMakeLove(HOGLIN, 0.6)` runs in both IDLE and FIGHT.
            goal_selector.add_goal(2, BreedGoal::new(0.6));
            // Vanilla `SetWalkTargetAwayFrom.pos(NEAREST_REPELLENT, 1.0, 8, true)`, an IDLE
            // behaviour.
            goal_selector.add_goal(
                3,
                Box::new(
                    AvoidRepellentGoal::new(
                        |mob| {
                            mob.cast_any()
                                .downcast_ref::<Self>()
                                .and_then(|hoglin| hoglin.nearest_repellent.load())
                        },
                        true,
                    )
                    .with_gate(|mob| mob.get_mob_entity().get_target().is_none()),
                ),
            );
            goal_selector.add_goal(
                4,
                Box::new(MeleeAttackGoal::new(1.0, true).with_attack_interval(|mob| {
                    hoglin_attack_interval(mob.get_mob_entity().living_entity.is_baby())
                })),
            );
            // Stands in for a vanilla stroll that ignores the no-action time.
            goal_selector.add_goal(
                5,
                Box::new(WanderAroundGoal::new(1.0).with_check_no_action_time(false)),
            );
            goal_selector.add_goal(
                6,
                LookAtEntityGoal::with_default(mob_weak.clone(), &EntityType::PLAYER, 8.0),
            );
            goal_selector.add_goal(7, Box::new(RandomLookAroundGoal::default()));

            let target_selector = &mob_arc.mob_entity.target_selector;
            target_selector.add_goal(1, Box::new(RevengeGoal::new(true)));
            // Vanilla `findNearestValidAttackTarget`: nothing while pacified.
            let hoglin_weak = Arc::downgrade(&mob_arc);
            target_selector.add_goal(
                2,
                Box::new(ActiveTargetGoal::new(
                    &mob_arc.mob_entity,
                    &EntityType::PLAYER,
                    10,
                    true,
                    false,
                    Some(move |_target: &LivingEntity, _world: &World| {
                        hoglin_weak
                            .upgrade()
                            .is_some_and(|hoglin| !hoglin.is_pacified())
                    }),
                )),
            );
        };

        mob_arc
    }

    /// Vanilla `Hoglin.canBeHunted`.
    pub fn can_be_hunted(&self) -> bool {
        !self.is_baby() && !self.cannot_be_hunted.load(Ordering::Relaxed)
    }

    /// Vanilla `HoglinAi.isPacified`.
    pub fn is_pacified(&self) -> bool {
        self.pacified_timer.load(Ordering::Relaxed) > 0
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

    /// Vanilla `HoglinSpecificSensor.findNearestRepellent`.
    fn refresh_nearest_repellent(&self) {
        let entity = self.get_entity();
        let nearest = find_nearest_block_by_manhattan_distance(
            &entity.world.load(),
            entity.block_pos.load(),
            REPELLENT_DETECTION_RANGE_HORIZONTAL,
            REPELLENT_DETECTION_RANGE_VERTICAL,
            |block, _state| block.has_tag(&tag::Block::MINECRAFT_HOGLIN_REPELLENTS),
        );
        self.nearest_repellent.store(nearest);
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

    // Vanilla `HoglinAi.getSoundForActivity`; Pumpkin's hoglin has no avoid activity.
    fn get_ambient_sound(&self) -> Option<Sound> {
        Some(if self.is_converting(&self.get_entity().world.load()) {
            Sound::EntityHoglinRetreat
        } else if self.mob_entity.get_target().is_some() {
            Sound::EntityHoglinAngry
        } else if self.nearest_repellent.load().is_some() {
            Sound::EntityHoglinRetreat
        } else {
            Sound::EntityHoglinAmbient
        })
    }

    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        if self.is_pacified() {
            self.pacified_timer.fetch_sub(1, Ordering::Relaxed);
        }
        let entity = self.get_entity();
        let tick = entity.world.load().get_world_age() + i64::from(entity.entity_id);
        if tick.rem_euclid(SENSOR_SCAN_INTERVAL) == 0 {
            self.refresh_nearest_repellent();
        }
        // Vanilla `BecomePassiveIfMemoryPresent(NEAREST_REPELLENT, 200)`.
        if self.nearest_repellent.load().is_some() && !self.is_pacified() {
            self.pacified_timer
                .store(REPELLENT_PACIFY_TIME, Ordering::Relaxed);
            self.set_mob_target(None, TargetReason::ForgotTarget);
        }
    }

    /// Vanilla `HoglinAi.wasHurtBy` forgets the `PACIFIED` memory.
    fn on_damage_with_context(
        &self,
        _damage_type: pumpkin_data::damage::DamageType,
        _source: Option<&dyn EntityBase>,
        _cause: Option<&dyn EntityBase>,
    ) -> bool {
        self.pacified_timer.store(0, Ordering::Relaxed);
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

    fn on_attack(&self, target: &dyn EntityBase) {
        // Vanilla HoglinBase.hurtAndThrowTarget: babies don't throw.
        if self.is_baby() {
            return;
        }
        throw_target(&self.mob_entity.living_entity, target);
    }

    fn blocked_by_item(&self, defender: &dyn EntityBase, _fully_blocked: bool) {
        if !self.is_baby() {
            throw_target(&self.mob_entity.living_entity, defender);
        }
    }
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
