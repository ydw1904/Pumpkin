use crate::entity::r#type::CreatureSpawnLevel;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Weak};

use pumpkin_data::attributes::Attributes;
use pumpkin_data::entity::{EntityStatus, EntityType};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::tracked_data;
use pumpkin_data::{damage::DamageType, sound::Sound};
use pumpkin_protocol::bedrock::server::ActorEventID;
use pumpkin_util::Difficulty;
use pumpkin_util::math::vector3::Vector3;

use crate::entity::ai::behavior::hunting::{self, Hunt};
use crate::entity::ai::control::move_control::{MoveControl, Operation};
use crate::entity::ai::control::{Control, MoveControlTrait};
use crate::entity::ai::goal::move_towards_restriction::MoveTowardsRestrictionGoal;
use crate::entity::ai::goal::{Controls, Goal};
use crate::entity::ai::pathfinder::{Navigator, node::PathType};
use crate::entity::ai::target_match::TargetMatch;
use crate::entity::custom_sound::CustomSound;
use crate::entity::living::LivingEntity;
use crate::entity::{
    Entity, EntityBase,
    ai::goal::{
        look_around::RandomLookAroundGoal, look_at_entity::LookAtEntityGoal,
        wander_around::WanderAroundGoal,
    },
    mob::{Mob, MobEntity},
};
use crate::world::World;

/// Vanilla `Guardian.ATTACK_TIME`.
const ATTACK_TIME: i32 = 80;
const RANDOM_STROLL_INTERVAL: i32 = 80;

const GUARDIAN_PREY: TargetMatch = TargetMatch::AnyOf(&[
    &EntityType::PLAYER,
    &EntityType::SQUID,
    &EntityType::GLOW_SQUID,
    &EntityType::AXOLOTL,
]);

/// Vanilla `GuardianAttackSelector`: nothing within 3 blocks.
fn outside_melee_range(hunter: &dyn Mob, target: &LivingEntity, _world: &World) -> bool {
    let hunter_pos = hunter.get_mob_entity().living_entity.entity.pos.load();
    target
        .entity
        .pos
        .load()
        .squared_distance_to_vec(&hunter_pos)
        > 9.0
}

/// Vanilla `Guardian.getWalkTargetValue`.
pub fn guardian_walk_target_value(
    level: &dyn CreatureSpawnLevel,
    pos: &pumpkin_util::math::position::BlockPos,
) -> f32 {
    if level.fluid(pos).has_tag(&tag::Fluid::MINECRAFT_WATER) {
        10.0 + level.pathfinding_cost_from_light_levels(pos)
    } else {
        crate::entity::mob::monster_walk_target_value(level, pos)
    }
}

/// Vanilla `Guardian.getAmbientSoundInterval`.
pub const AMBIENT_SOUND_INTERVAL: i32 = 160;

pub const GUARDIAN_HUNTS: &[Hunt] =
    &[Hunt::new(1, GUARDIAN_PREY, true).matching(outside_melee_range)];

/// The state vanilla `Guardian` keeps beside the shared mob fields, shared with its goals and
/// move control.
#[derive(Default)]
pub struct GuardianState {
    moving: AtomicBool,
    active_attack_target: AtomicI32,
    /// Vanilla `randomStrollGoal.trigger()`.
    stroll_trigger: Arc<AtomicBool>,
}

impl GuardianState {
    pub fn is_moving(&self) -> bool {
        self.moving.load(Ordering::Relaxed)
    }

    fn set_moving(&self, entity: &Entity, moving: bool) {
        self.moving.store(moving, Ordering::Relaxed);
        entity.set_synced_data(tracked_data::guardian::DATA_ID_MOVING, moving);
    }

    pub fn has_active_attack_target(&self) -> bool {
        self.active_attack_target.load(Ordering::Relaxed) != 0
    }

    fn set_active_attack_target(&self, entity: &Entity, entity_id: i32) {
        self.active_attack_target
            .store(entity_id, Ordering::Relaxed);
        entity.set_synced_data(tracked_data::guardian::DATA_ID_ATTACK_TARGET, entity_id);
    }

    fn trigger_stroll(&self) {
        self.stroll_trigger.store(true, Ordering::Relaxed);
    }
}

/// Vanilla `Guardian`'s constructor and `registerGoals`, shared with the elder guardian.
pub fn init_guardian(
    mob_entity: &mut MobEntity,
    state: &Arc<GuardianState>,
    mob_weak: &Weak<dyn Mob>,
    elder: bool,
    stroll_interval: i32,
) {
    // Vanilla `Guardian.defineSynchedData`, so the defaults aren't sent.
    let synched_data = &mob_entity.living_entity.entity.synched_data;
    synched_data.define(tracked_data::guardian::DATA_ID_MOVING, false);
    synched_data.define(tracked_data::guardian::DATA_ID_ATTACK_TARGET, 0i32);
    *mob_entity
        .move_control
        .get_mut()
        .unwrap_or_else(std::sync::PoisonError::into_inner) =
        Box::new(GuardianMoveControl::new(state.clone()));
    let navigator = mob_entity
        .navigator
        .get_mut()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *navigator = Navigator::water_bound(false);
    navigator.set_pathfinding_malus(PathType::Water, 0.0);

    let move_flags = Controls::MOVE | Controls::LOOK;
    let goal_selector = &mob_entity.goals_selector;
    goal_selector.add_goal(4, Box::new(GuardianAttackGoal::new(state.clone(), elder)));
    goal_selector.add_goal(
        5,
        Box::new(MoveTowardsRestrictionGoal::new(1.0).with_controls(move_flags)),
    );
    goal_selector.add_goal(
        7,
        Box::new(
            WanderAroundGoal::with_interval(1.0, stroll_interval)
                .with_controls(move_flags)
                .with_trigger(state.stroll_trigger.clone()),
        ),
    );
    goal_selector.add_goal(
        8,
        LookAtEntityGoal::with_default(mob_weak.clone(), &EntityType::PLAYER, 8.0),
    );
    goal_selector.add_goal(
        8,
        Box::new(LookAtEntityGoal::matching(
            mob_weak.clone(),
            TargetMatch::GUARDIAN_LIKE,
            12.0,
            0.01,
            false,
        )),
    );
    goal_selector.add_goal(9, Box::new(RandomLookAroundGoal::default()));

    hunting::apply(&mob_entity.target_selector, mob_entity, GUARDIAN_HUNTS);
}

/// Vanilla `Guardian.aiStep`, the part before `super.aiStep()`.
pub fn guardian_ai_step(mob: &dyn Mob, state: &GuardianState) {
    let entity = mob.get_entity();
    if !entity.is_alive() {
        return;
    }
    if entity.is_in_water() {
        entity.air_supply.store(300, Ordering::Relaxed);
    } else if entity.on_ground.load(Ordering::Relaxed) {
        let rng = mob.get_random();
        let x = f64::from((rng.random::<f32>() * 2.0 - 1.0) * 0.4);
        let z = f64::from((rng.random::<f32>() * 2.0 - 1.0) * 0.4);
        entity
            .velocity
            .store(entity.velocity.load() + Vector3::new(x, 0.5, z));
        entity.yaw.store(rng.random::<f32>() * 360.0);
        entity.on_ground.store(false, Ordering::Relaxed);
        entity.velocity_dirty.store(true, Ordering::Relaxed);
    }
    if state.has_active_attack_target() {
        entity.yaw.store(entity.head_yaw.load());
    }
}

/// Vanilla `Guardian.hurtServer`, the part before `super.hurtServer`: thorns while still and
/// a fresh random stroll.
pub fn guardian_pre_damage(
    guardian: &dyn EntityBase,
    state: &GuardianState,
    damage_type: DamageType,
    source: Option<&dyn EntityBase>,
) {
    if !state.is_moving()
        && !damage_type.has_tag(&tag::DamageType::MINECRAFT_AVOIDS_GUARDIAN_THORNS)
        && damage_type != DamageType::THORNS
        && let Some(direct) = source
        && direct.get_living_entity().is_some()
    {
        direct.damage_with_context(
            direct,
            2.0,
            DamageType::THORNS,
            None,
            Some(guardian),
            Some(guardian),
        );
    }
    state.trigger_stroll();
}

/// Vanilla `Guardian.travelInWater`.
pub fn guardian_travel_in_water(mob: &dyn Mob, caller: &dyn EntityBase, state: &GuardianState) {
    let mob_entity = mob.get_mob_entity();
    let living = &mob_entity.living_entity;
    let entity = &living.entity;
    entity.update_velocity_from_input(living.movement_input.load(), 0.1);
    living.make_move(caller);
    let mut velocity = entity.velocity.load() * 0.9;
    if !state.is_moving() && mob_entity.get_target().is_none() {
        velocity.y -= 0.005;
    }
    entity.velocity.store(velocity);
}

pub struct GuardianEntity {
    pub mob_entity: MobEntity,
    state: Arc<GuardianState>,
}

impl GuardianEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let state = Arc::new(GuardianState::default());
        Arc::new_cyclic(|weak: &Weak<Self>| {
            let mob_weak: Weak<dyn Mob> = weak.clone();
            let mut mob_entity = MobEntity::new(entity);
            init_guardian(
                &mut mob_entity,
                &state,
                &mob_weak,
                false,
                RANDOM_STROLL_INTERVAL,
            );
            Self { mob_entity, state }
        })
    }
}

impl CustomSound for GuardianEntity {
    fn hurt_sound(&self) -> Option<Sound> {
        let entity = self.get_entity();
        let is_water = entity.touching_water.load(Ordering::Relaxed);
        Some(if is_water {
            Sound::EntityGuardianHurt
        } else {
            Sound::EntityGuardianHurtLand
        })
    }

    fn death_sound(&self) -> Option<Sound> {
        let entity = self.get_entity();
        let is_water = entity.touching_water.load(Ordering::Relaxed);
        Some(if is_water {
            Sound::EntityGuardianDeath
        } else {
            Sound::EntityGuardianDeathLand
        })
    }
}

impl Mob for GuardianEntity {
    fn get_ambient_sound_interval(&self) -> i32 {
        AMBIENT_SOUND_INTERVAL
    }

    fn get_ambient_sound(&self) -> Option<Sound> {
        Some(if self.get_entity().is_in_water() {
            Sound::EntityGuardianAmbient
        } else {
            Sound::EntityGuardianAmbientLand
        })
    }

    fn get_walk_target_value(
        &self,
        level: &dyn CreatureSpawnLevel,
        pos: &pumpkin_util::math::position::BlockPos,
    ) -> f32 {
        guardian_walk_target_value(level, pos)
    }

    /// Vanilla `Guardian.checkSpawnObstruction`.
    fn check_spawn_obstruction(&self, level: &dyn CreatureSpawnLevel) -> bool {
        crate::entity::mob::is_unobstructed(self, level)
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn get_max_look_pitch_change(&self) -> f32 {
        180.0
    }

    fn before_mob_ai_step(&self) {
        guardian_ai_step(self, &self.state);
    }

    fn travel_in_water(&self, caller: &dyn EntityBase, _gravity: f64, _falling: bool) {
        guardian_travel_in_water(self, caller, &self.state);
    }

    fn pre_damage(&self, damage_type: DamageType, source: Option<&dyn EntityBase>) -> bool {
        guardian_pre_damage(self, &self.state, damage_type, source);
        true
    }
}

/// Vanilla `Guardian.GuardianAttackGoal`: the beam.
pub struct GuardianAttackGoal {
    state: Arc<GuardianState>,
    attack_time: i32,
    elder: bool,
}

impl GuardianAttackGoal {
    #[must_use]
    pub const fn new(state: Arc<GuardianState>, elder: bool) -> Self {
        Self {
            state,
            attack_time: 0,
            elder,
        }
    }

    /// Vanilla `Guardian.getAttackDuration`, which the elder guardian shortens.
    const fn attack_duration(&self) -> i32 {
        if self.elder {
            crate::entity::mob::elder_guardian::ATTACK_DURATION
        } else {
            ATTACK_TIME
        }
    }
}

impl Goal for GuardianAttackGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        mob.get_mob_entity()
            .get_target()
            .is_some_and(|target| target.get_entity().is_alive())
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        if !self.can_start(mob) {
            return false;
        }
        if self.elder {
            return true;
        }
        let pos = mob.get_entity().pos.load();
        mob.get_mob_entity().get_target().is_some_and(|target| {
            target.get_entity().pos.load().squared_distance_to_vec(&pos) > 9.0
        })
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.attack_time = -10;
        let mob_entity = mob.get_mob_entity();
        mob_entity.steered().navigator().stop();
        if let Some(target) = mob_entity.get_target() {
            mob_entity
                .look_control
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .look_at_entity_with_range(&target, 90.0, 90.0);
        }
        mob.get_entity()
            .velocity_dirty
            .store(true, Ordering::Relaxed);
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.state.set_active_attack_target(mob.get_entity(), 0);
        mob.get_mob_entity().set_target(None);
        self.state.trigger_stroll();
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let mob_entity = mob.get_mob_entity();
        let Some(target) = mob_entity.get_target() else {
            return;
        };
        mob_entity.steered().navigator().stop();
        mob_entity
            .look_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .look_at_entity_with_range(&target, 90.0, 90.0);
        if !mob.has_line_of_sight(target.get_entity()) {
            mob_entity.set_target(None);
            return;
        }

        self.attack_time += 1;
        let entity = mob.get_entity();
        if self.attack_time == 0 {
            self.state
                .set_active_attack_target(entity, target.get_entity().entity_id);
            if !entity.is_silent() {
                entity.world.load().send_entity_status(
                    entity,
                    EntityStatus::GuardianAttackSound,
                    ActorEventID::None,
                );
            }
        } else if self.attack_time >= self.attack_duration() {
            let mut magic_damage = 1.0;
            if entity.world.load().level_info.load().difficulty == Difficulty::Hard {
                magic_damage += 2.0;
            }
            if self.elder {
                magic_damage += 2.0;
            }
            target.damage_with_context(
                target.as_ref(),
                magic_damage,
                DamageType::INDIRECT_MAGIC,
                None,
                Some(entity),
                Some(entity),
            );
            mob.do_hurt_target(target.as_ref());
            mob_entity.set_target(None);
        }
    }

    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK
    }
}

/// Vanilla `Guardian.GuardianMoveControl`.
pub struct GuardianMoveControl {
    inner: MoveControl,
    state: Arc<GuardianState>,
}

impl GuardianMoveControl {
    #[must_use]
    pub fn new(state: Arc<GuardianState>) -> Self {
        Self {
            inner: MoveControl::default(),
            state,
        }
    }
}

impl Control for GuardianMoveControl {}

impl MoveControlTrait for GuardianMoveControl {
    fn tick(&mut self, mob: &dyn Mob) {
        let mob_entity = mob.get_mob_entity();
        let living = &mob_entity.living_entity;
        let entity = &living.entity;
        if self.inner.operation != Operation::MoveTo || mob_entity.steered().navigator().is_done() {
            mob_entity.set_speed(0.0);
            self.state.set_moving(entity, false);
            return;
        }

        let pos = entity.pos.load();
        let delta = Vector3::new(
            self.inner.wanted_x - pos.x,
            self.inner.wanted_y - pos.y,
            self.inner.wanted_z - pos.z,
        );
        let length = delta.length();
        let xd = delta.x / length;
        let yd = delta.y / length;
        let zd = delta.z / length;
        let y_rot_d =
            (delta.z.atan2(delta.x) * 180.0 / f64::from(std::f32::consts::PI)) as f32 - 90.0;
        let yaw = MoveControl::rotlerp(entity.yaw.load(), y_rot_d, MoveControl::MAX_TURN);
        entity.yaw.store(yaw);
        entity.body_yaw.store(yaw);
        let target_speed = (self.inner.speed_modifier
            * living.get_attribute_value(&Attributes::MOVEMENT_SPEED))
            as f32;
        let speed = living.get_speed();
        let new_speed = speed + 0.125 * (target_speed - speed);
        mob_entity.set_speed(new_speed);

        let phase = f64::from(
            entity
                .tick_count
                .load(Ordering::Relaxed)
                .wrapping_add(entity.entity_id),
        );
        let push = (phase * 0.5).sin() * 0.05;
        let (sin, cos) = f64::from(yaw * (std::f64::consts::PI / 180.0) as f32).sin_cos();
        let y_push = (phase * 0.75).sin() * 0.05;
        entity.velocity.store(
            entity.velocity.load()
                + Vector3::new(
                    push * cos,
                    y_push * (sin + cos) * 0.25 + f64::from(new_speed) * yd * 0.1,
                    push * sin,
                ),
        );

        let new_look = Vector3::new(
            pos.x + xd * 2.0,
            entity.get_eye_y() + yd / length,
            pos.z + zd * 2.0,
        );
        let mut look_control = mob_entity
            .look_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let old_look = look_control.wanted_look().unwrap_or(new_look);
        look_control.look_at_with_range(
            old_look.x + 0.125 * (new_look.x - old_look.x),
            old_look.y + 0.125 * (new_look.y - old_look.y),
            old_look.z + 0.125 * (new_look.z - old_look.z),
            10.0,
            40.0,
        );
        drop(look_control);
        self.state.set_moving(entity, true);
    }

    fn set_wanted_position(&mut self, x: f64, y: f64, z: f64, speed_modifier: f64) {
        self.inner.set_wanted_position(x, y, z, speed_modifier);
    }

    fn strafe(&mut self, forwards: f32, right: f32) {
        self.inner.strafe(forwards, right);
    }

    fn has_wanted(&self) -> bool {
        self.inner.has_wanted()
    }
}
