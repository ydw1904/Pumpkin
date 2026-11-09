use crate::entity::r#type::CreatureSpawnLevel;
use std::sync::{
    Arc, Mutex, PoisonError,
    atomic::{AtomicBool, Ordering},
};

use pumpkin_data::BlockState;
use pumpkin_data::{
    damage::DamageType,
    effect::StatusEffect,
    entity::{EntityPose, EntityStatus, EntityType},
    potion::Effect,
    sound::Sound,
    tag::{self, Taggable},
    tracked_data,
};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::bedrock::server::actor_event::ActorEventID;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::math::{boundingbox::EntityDimensions, position::BlockPos};
use pumpkin_util::{GameMode, math::vector3::Vector3};

use crate::entity::{
    Entity, EntityBase,
    ai::{
        brain::{
            Brain, BrainState, memory::PackedMemories, memory::types, pending::PendingBrainUpdates,
        },
        pathfinder::node::PathType,
    },
    mob::{
        Mob, MobEntity,
        spawn::{EntitySpawnReason, SpawnGroupData},
        warden_anger::{AngerLevel, AngerManagement},
    },
    vibration::VibrationData,
};
use crate::plugin::api::events::entity::entity_target::TargetReason;
use crate::world::World;

pub mod ai;
mod vibration;

const DARKNESS_DISPLAY_LIMIT: i32 = 200;
const DARKNESS_DURATION: i32 = 260;
const DARKNESS_RADIUS: f64 = 20.0;
const DARKNESS_INTERVAL: i32 = 120;
const ANGERMANAGEMENT_TICK_DELAY: i32 = 20;
const DEFAULT_ANGER: i32 = 35;
const PROJECTILE_ANGER: i32 = 10;
const ON_HURT_ANGER_BOOST: i32 = 20;
const RECENT_PROJECTILE_TICK_THRESHOLD: i64 = 100;
const TOUCH_COOLDOWN_TICKS: i64 = 20;
const VIBRATION_COOLDOWN_TICKS: i64 = 40;
const PROJECTILE_ANGER_DISTANCE: f64 = 30.0;
/// The attacker counts as a target from this close even for an indirect hit.
const HURT_TARGET_DISTANCE: f64 = 5.0;
/// Vanilla `Warden.getDefaultDimensions` while digging or emerging.
const DIGGING_HEIGHT: f32 = 1.0;
/// Vanilla `EntityDimensions.fixed` eye height.
const FIXED_EYE_HEIGHT_RATIO: f32 = 0.85;
/// Vanilla `Warden.nextStep`: a step sound every 0.55 blocks walked.
const STEP_DISTANCE: f32 = 0.55;

pub struct WardenEntity {
    pub mob_entity: MobEntity,
    anger_management: Mutex<AngerManagement>,
    vibration: Mutex<VibrationData>,
    /// `VIBRATION_COOLDOWN` as of the last brain access, for listeners called during the
    /// warden's own brain tick.
    vibration_cooldown: AtomicBool,
    pending_brain_updates: PendingBrainUpdates<Self>,
}

impl WardenEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        // Vanilla `Warden.defineSynchedData`.
        entity.synched_data.define(
            pumpkin_data::tracked_data::warden::CLIENT_ANGER_LEVEL,
            VarInt(0),
        );
        let mut mob_entity = MobEntity::new(entity);
        let navigator = mob_entity
            .navigator
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner);
        navigator.set_can_float(true);
        navigator.set_xz_step_cost(true);
        for (path_type, malus) in [
            (PathType::UnpassableRail, 0.0),
            (PathType::DamageOther, 8.0),
            (PathType::PowderSnow, 8.0),
            (PathType::Lava, 8.0),
            (PathType::DamageFire, 0.0),
            (PathType::DangerFire, 0.0),
        ] {
            navigator.set_pathfinding_malus(path_type, malus);
        }
        Arc::new(Self {
            mob_entity,
            anger_management: Mutex::new(AngerManagement::default()),
            vibration: Mutex::new(VibrationData::default()),
            vibration_cooldown: AtomicBool::new(false),
            pending_brain_updates: PendingBrainUpdates::default(),
        })
    }

    /// Vanilla `Warden.finalizeSpawn` for `EntitySpawnReason.TRIGGERED`, used by shriekers
    /// after spawning the warden.
    pub fn emerge(&self) {
        self.with_own_brain(|warden, brain| {
            brain.set_with_expiry(types::DIG_COOLDOWN, (), ai::DIGGING_COOLDOWN);
            warden.start_emerging(brain);
        });
    }

    fn start_emerging(&self, brain: &mut BrainState) {
        self.set_warden_pose(EntityPose::Emerging);
        brain.set_with_expiry(types::IS_EMERGING, (), i64::from(ai::EMERGE_DURATION));
        self.play_entity_sound(Sound::EntityWardenAgitated, 5.0, 1.0);
    }

    fn is_digging_or_emerging(&self) -> bool {
        Self::is_digging_or_emerging_pose(self.mob_entity.living_entity.entity.pose.load())
    }

    const fn is_digging_or_emerging_pose(pose: EntityPose) -> bool {
        matches!(pose, EntityPose::Digging | EntityPose::Emerging)
    }

    /// `set_pose` with vanilla's emerging spawn data and the short digging box.
    pub(super) fn set_warden_pose(&self, pose: EntityPose) {
        let entity = &self.mob_entity.living_entity.entity;
        let was_low = Self::is_digging_or_emerging_pose(entity.pose.load());
        entity.set_pose(pose);
        // Vanilla `Warden.getAddEntityPacket`.
        entity
            .data
            .store(i32::from(pose == EntityPose::Emerging), Ordering::Relaxed);
        let low = Self::is_digging_or_emerging_pose(pose);
        if low && !was_low {
            let width = entity.entity_type.dimension[0];
            entity.set_default_dimensions(EntityDimensions::new(
                width,
                DIGGING_HEIGHT,
                DIGGING_HEIGHT * FIXED_EYE_HEIGHT_RATIO,
            ));
        } else if was_low && !low {
            entity.clear_default_dimensions();
        }
    }

    pub(super) fn has_pose(&self, pose: EntityPose) -> bool {
        self.mob_entity.living_entity.entity.pose.load() == pose
    }

    fn anger(&self) -> std::sync::MutexGuard<'_, AngerManagement> {
        self.anger_management
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Runs `update` on the brain now, or right after the brain tick that holds it.
    fn with_own_brain(&self, update: impl FnOnce(&Self, &mut BrainState) + Send + 'static) {
        self.pending_brain_updates
            .update_then(self, update, |warden, brain| {
                warden.after_brain_access(brain);
            });
    }

    /// Keeps the state read outside the brain in step with it.
    fn after_brain_access(&self, brain: &BrainState) {
        self.vibration_cooldown.store(
            brain.has_memory_value(types::VIBRATION_COOLDOWN.id()),
            Ordering::Relaxed,
        );
        let target = brain.get(types::ATTACK_TARGET).cloned();
        let current = self.mob_entity.get_target();
        if current.as_ref().map(|t| t.get_entity().entity_id)
            != target.as_ref().map(|t| t.get_entity().entity_id)
        {
            let reason = if target.is_some() {
                TargetReason::Angry
            } else {
                TargetReason::ForgotTarget
            };
            // A plugin cancelling a new target keeps the warden from attacking it.
            if !self.set_mob_target(target.clone(), reason) && target.is_some() {
                self.mob_entity.set_target(None);
            }
        }
    }

    /// Vanilla `Warden.canTargetEntity`.
    pub fn can_target_entity(&self, entity: &dyn EntityBase) -> bool {
        let Some(living) = entity.get_living_entity() else {
            return false;
        };
        let target = entity.get_entity();
        let world = self.mob_entity.living_entity.entity.world.load();
        if !Arc::ptr_eq(&world, &target.world.load()) {
            return false;
        }
        if entity
            .get_player()
            .is_some_and(|player| player.is_creative() || player.is_spectator())
        {
            return false;
        }
        !(self as &dyn EntityBase).is_allied_to(entity)
            && target.entity_type != &EntityType::ARMOR_STAND
            && target.entity_type != &EntityType::WARDEN
            && !target.invulnerable.load(Ordering::Relaxed)
            && !living.is_dead_or_dying()
            && world
                .worldborder
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .contains_box(&target.bounding_box.load())
    }

    fn active_anger(&self, brain: &BrainState) -> i32 {
        let target = brain.get(types::ATTACK_TARGET);
        self.anger().get_active_anger(target.map(AsRef::as_ref))
    }

    /// Vanilla `Warden.getAngerLevel`.
    pub fn anger_level(&self, brain: &BrainState) -> AngerLevel {
        AngerLevel::by_anger(self.active_anger(brain))
    }

    /// Vanilla `Warden.getEntityAngryAt`.
    pub fn entity_angry_at(&self, brain: &BrainState) -> Option<Arc<dyn EntityBase>> {
        if !self.anger_level(brain).is_angry() {
            return None;
        }
        self.active_suspect()
    }

    /// Vanilla `AngerManagement.getActiveEntity`.
    fn active_suspect(&self) -> Option<Arc<dyn EntityBase>> {
        self.anger()
            .get_active_entity(|entity| self.can_target_entity(entity))
    }

    pub(super) fn clear_anger(&self, entity: &dyn EntityBase) {
        self.anger().clear_anger(entity);
    }

    /// Vanilla `Warden.increaseAngerAt`.
    pub(super) fn increase_anger_in(
        &self,
        brain: &mut BrainState,
        entity: &Arc<dyn EntityBase>,
        amount: i32,
        play_sound: bool,
    ) {
        if self.mob_entity.is_no_ai() || !self.can_target_entity(entity.as_ref()) {
            return;
        }
        ai::set_dig_cooldown(brain);
        let maybe_switch_target = brain
            .get(types::ATTACK_TARGET)
            .is_none_or(|target| target.get_player().is_none());
        let new_anger = self.anger().increase_anger(entity, amount);
        if entity.get_player().is_some()
            && maybe_switch_target
            && AngerLevel::by_anger(new_anger).is_angry()
        {
            brain.erase(types::ATTACK_TARGET.id());
        }
        if play_sound {
            self.play_listening_sound(brain);
        }
    }

    /// Vanilla `Warden.setAttackTarget`.
    pub(super) fn set_attack_target(brain: &mut BrainState, target: Arc<dyn EntityBase>) {
        brain.erase(types::ROAR_TARGET.id());
        brain.set(types::ATTACK_TARGET, target);
        brain.erase(types::CANT_REACH_WALK_TARGET_SINCE.id());
        ai::set_sonic_boom_cooldown(brain, ai::SONIC_BOOM_TARGET_COOLDOWN);
    }

    fn play_listening_sound(&self, brain: &BrainState) {
        if !self.has_pose(EntityPose::Roaring) {
            self.play_entity_sound(
                self.anger_level(brain).listening_sound(),
                10.0,
                self.mob_entity.living_entity.get_pitch(),
            );
        }
    }

    fn sync_client_anger_level(&self, brain: &BrainState) {
        self.mob_entity.living_entity.entity.set_synced_data(
            tracked_data::warden::CLIENT_ANGER_LEVEL,
            VarInt(self.active_anger(brain)),
        );
    }

    /// Vanilla `Warden.VibrationUser.canReceiveVibration`.
    fn can_receive_vibration(
        &self,
        world: &World,
        pos: BlockPos,
        source: Option<&dyn EntityBase>,
    ) -> bool {
        let cooldown = self
            .mob_entity
            .try_with_brain(self, |brain| {
                brain.has_memory_value(types::VIBRATION_COOLDOWN.id())
            })
            .unwrap_or_else(|| self.vibration_cooldown.load(Ordering::Relaxed));
        !self.mob_entity.is_no_ai()
            && !self.mob_entity.living_entity.is_dead_or_dying()
            && !cooldown
            && !self.is_digging_or_emerging()
            && world
                .worldborder
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .contains_block(pos.0.x, pos.0.z)
            && !source.is_some_and(|source| {
                source.get_living_entity().is_some() && !self.can_target_entity(source)
            })
    }

    /// Vanilla `Warden.VibrationUser.onReceiveVibration`.
    fn on_receive_vibration(
        &self,
        brain: &mut BrainState,
        world: &World,
        pos: BlockPos,
        source: Option<&Arc<dyn EntityBase>>,
        projectile_owner: Option<&Arc<dyn EntityBase>>,
    ) {
        if self.mob_entity.living_entity.is_dead_or_dying() {
            return;
        }
        brain.set_with_expiry(types::VIBRATION_COOLDOWN, (), VIBRATION_COOLDOWN_TICKS);
        let entity = self.get_entity();
        world.send_entity_status(
            entity,
            EntityStatus::TendrilsShiver,
            ActorEventID::VibrationDetected,
        );
        self.play_entity_sound(
            Sound::EntityWardenTendrilClicks,
            5.0,
            self.mob_entity.living_entity.get_pitch(),
        );
        let mut suspicious_pos = pos;
        if let Some(owner) = projectile_owner {
            if self.closer_than(owner.as_ref(), PROJECTILE_ANGER_DISTANCE) {
                if brain.has_memory_value(types::RECENT_PROJECTILE.id()) {
                    if self.can_target_entity(owner.as_ref()) {
                        suspicious_pos = owner.get_entity().block_pos.load();
                    }
                    self.increase_anger_in(brain, owner, DEFAULT_ANGER, true);
                } else {
                    self.increase_anger_in(brain, owner, PROJECTILE_ANGER, true);
                }
            }
            brain.set_with_expiry(
                types::RECENT_PROJECTILE,
                (),
                RECENT_PROJECTILE_TICK_THRESHOLD,
            );
        } else if let Some(source) = source {
            self.increase_anger_in(brain, source, DEFAULT_ANGER, true);
        }
        if !self.anger_level(brain).is_angry() {
            let active = self.active_suspect();
            if projectile_owner.is_some()
                || active.as_ref().is_none_or(|active| {
                    source.as_ref().is_some_and(|source| {
                        active.get_entity().entity_id == source.get_entity().entity_id
                    })
                })
            {
                ai::set_disturbance_location(self, brain, suspicious_pos);
            }
        }
    }

    /// Vanilla `Entity.closerThan(entity, distance)`.
    fn closer_than(&self, other: &dyn EntityBase, distance: f64) -> bool {
        self.get_entity()
            .pos
            .load()
            .squared_distance_to_vec(&other.get_entity().pos.load())
            < distance * distance
    }

    /// Vanilla `Entity.closerThan(entity, horizontal, vertical)`.
    pub(super) fn closer_than_xz_y(
        &self,
        other: &dyn EntityBase,
        horizontal: f64,
        vertical: f64,
    ) -> bool {
        let delta = other.get_entity().pos.load() - self.get_entity().pos.load();
        delta.x * delta.x + delta.z * delta.z < horizontal * horizontal
            && delta.y * delta.y < vertical * vertical
    }

    /// Vanilla `Warden.applyDarknessAround` through `MobEffectUtil.addEffectToPlayersAround`.
    pub fn apply_darkness_around(
        world: &World,
        position: Vector3<f64>,
        source: Option<&dyn EntityBase>,
        radius: f64,
    ) {
        for player in world.get_nearby_players(position, radius) {
            if !matches!(
                player.gamemode.load(),
                GameMode::Survival | GameMode::Adventure
            ) || source.is_some_and(|source| source.is_allied_to(player.as_ref()))
                // `closerThan` is strict.
                || player.get_entity().pos.load().squared_distance_to_vec(&position)
                    >= radius * radius
            {
                continue;
            }
            let current = player.living_entity.get_effect(&StatusEffect::DARKNESS);
            if current.is_some_and(|effect| {
                effect.duration < 0 || effect.duration >= DARKNESS_DISPLAY_LIMIT
            }) {
                continue;
            }
            let darkness = Effect {
                effect_type: &StatusEffect::DARKNESS,
                duration: DARKNESS_DURATION,
                amplifier: 0,
                ambient: false,
                show_particles: false,
                show_icon: false,
                blend: true,
            };
            player
                .living_entity
                .add_effect_with_source(darkness, source);
        }
    }

    /// Vanilla `Warden.hurtServer` after the shared damage path.
    fn on_hurt(&self, source: Option<&dyn EntityBase>, cause: Option<&dyn EntityBase>) {
        if self.mob_entity.is_no_ai() || self.is_digging_or_emerging() {
            return;
        }
        let Some(cause) = cause else {
            return;
        };
        let world = self.get_entity().world.load();
        let cause_id = cause.get_entity().entity_id;
        let Some(attacker) = world
            .get_player_by_id(cause_id)
            .map(|player| player as Arc<dyn EntityBase>)
            .or_else(|| world.get_entity_by_id(cause_id))
        else {
            return;
        };
        let direct = source.is_some_and(|source| source.get_entity().entity_id == cause_id);
        let close = self.closer_than(attacker.as_ref(), HURT_TARGET_DISTANCE);
        self.with_own_brain(move |warden, brain| {
            warden.increase_anger_in(
                brain,
                &attacker,
                AngerLevel::Angry.minimum_anger() + ON_HURT_ANGER_BOOST,
                false,
            );
            if !brain.has_memory_value(types::ATTACK_TARGET.id())
                && attacker.get_living_entity().is_some()
                && (direct || close)
            {
                Self::set_attack_target(brain, attacker);
            }
        });
    }
}

impl Mob for WardenEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn make_brain(&self, packed: &PackedMemories) -> Brain {
        ai::make_brain(self, packed)
    }

    fn get_ambient_sound(&self) -> Option<Sound> {
        if self.has_pose(EntityPose::Roaring) || self.is_digging_or_emerging() {
            return None;
        }
        let target = self.mob_entity.get_target();
        let anger = self.anger().get_active_anger(target.as_deref());
        Some(AngerLevel::by_anger(anger).ambient_sound())
    }

    fn as_custom_sound(&self) -> Option<&dyn crate::entity::custom_sound::CustomSound> {
        Some(self)
    }

    /// Vanilla `Warden.getWalkTargetValue`.
    fn get_walk_target_value(&self, _level: &dyn CreatureSpawnLevel, _pos: &BlockPos) -> f32 {
        0.0
    }

    fn mob_is_pushable(&self) -> bool {
        !self.is_digging_or_emerging() && self.mob_entity.living_entity.is_pushable()
    }

    /// Vanilla `Warden.doPush`.
    fn mob_do_push(&self, other: &dyn EntityBase) {
        if self.mob_entity.is_no_ai() {
            return;
        }
        let world = self.get_entity().world.load();
        let other_id = other.get_entity().entity_id;
        let Some(other) = world
            .get_player_by_id(other_id)
            .map(|player| player as Arc<dyn EntityBase>)
            .or_else(|| world.get_entity_by_id(other_id))
        else {
            return;
        };
        self.with_own_brain(move |warden, brain| {
            if brain.has_memory_value(types::TOUCH_COOLDOWN.id()) {
                return;
            }
            brain.set_with_expiry(types::TOUCH_COOLDOWN, (), TOUCH_COOLDOWN_TICKS);
            warden.increase_anger_in(brain, &other, DEFAULT_ANGER, true);
            ai::set_disturbance_location(warden, brain, other.get_entity().block_pos.load());
        });
    }

    fn mob_dampens_vibrations(&self) -> bool {
        true
    }

    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {
        self.play_entity_sound(Sound::EntityWardenStep, 10.0, 1.0);
    }

    fn mob_next_step(&self) -> f32 {
        self.get_entity().move_dist.load() + STEP_DISTANCE
    }

    fn mob_can_ride(&self) -> bool {
        false
    }

    fn mob_ignores_explosion(&self) -> bool {
        self.is_digging_or_emerging()
    }

    fn check_spawn_obstruction(&self, level: &dyn CreatureSpawnLevel) -> bool {
        let entity = &self.mob_entity.living_entity.entity;
        let bounding_box = entity.bounding_box.load();
        !level.contains_any_liquid(bounding_box)
            && level.entities_at_box(&bounding_box).is_empty()
            && level.is_space_empty(bounding_box)
    }

    fn can_attack(&self, target: &dyn EntityBase) -> bool {
        self.can_target_entity(target)
    }

    fn remove_when_far_away(&self, _distance_sq: f64) -> bool {
        false
    }

    fn finalize_spawn(
        &self,
        _world: &Arc<World>,
        reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        self.with_own_brain(move |warden, brain| {
            brain.set_with_expiry(types::DIG_COOLDOWN, (), ai::DIGGING_COOLDOWN);
            if reason == EntitySpawnReason::Triggered {
                warden.start_emerging(brain);
            }
        });
        self.finalize_spawn_super(group_data)
    }

    fn mob_hurt_server(
        &self,
        caller: &dyn EntityBase,
        amount: f32,
        damage_type: DamageType,
        position: Option<Vector3<f64>>,
        source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> Option<bool> {
        // Vanilla `Warden.isInvulnerableTo`.
        let was_hurt = (!self.is_digging_or_emerging()
            || damage_type.has_tag(&tag::DamageType::MINECRAFT_BYPASSES_INVULNERABILITY))
            && self.pre_damage_with_context(damage_type, source, cause)
            && self.mob_entity.living_entity.damage_with_context(
                caller,
                self.modify_incoming_damage(amount, damage_type),
                damage_type,
                position,
                source,
                cause,
            )
            && self.on_damage_with_context(damage_type, source, cause);
        self.on_hurt(source, cause);
        Some(was_hurt)
    }

    /// Vanilla `Warden.doHurtTarget`.
    fn do_hurt_target(&self, target: &dyn EntityBase) -> bool {
        let entity = self.get_entity();
        entity.world.load().send_entity_status(
            entity,
            EntityStatus::StartAttacking,
            ActorEventID::StartAttacking,
        );
        self.play_entity_sound(
            Sound::EntityWardenAttackImpact,
            10.0,
            self.mob_entity.living_entity.get_pitch(),
        );
        self.with_own_brain(|_, brain| {
            ai::set_sonic_boom_cooldown(brain, ai::SONIC_BOOM_COOLDOWN);
        });
        let damaged = self.mob_entity.try_attack(entity, target);
        if damaged {
            self.on_attack(target);
        }
        damaged
    }

    fn mob_tick(&self, _caller: &dyn EntityBase) {
        let entity = self.get_entity();
        let world = entity.world.load_full();
        let received = self
            .vibration
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .tick(
                &world,
                entity.entity_id,
                entity.get_eye_height() as f32,
                entity.get_eye_pos(),
            );
        if let Some(vibration) = received {
            let source = vibration.source_entity(&world);
            let owner = vibration.projectile_owner(&world);
            let pos = BlockPos::floored_v(vibration.pos);
            let world = world.clone();
            self.with_own_brain(move |warden, brain| {
                warden.on_receive_vibration(brain, &world, pos, source.as_ref(), owner.as_ref());
            });
        }
        if self.mob_entity.persistence_required.load(Ordering::Relaxed)
            || self.requires_custom_persistence()
        {
            self.with_own_brain(|_, brain| ai::set_dig_cooldown(brain));
        }
    }

    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        let entity = self.get_entity();
        let world = entity.world.load_full();
        let time = world.get_world_age();
        self.mob_entity.with_brain(self, |brain| {
            brain.tick(&world, self, time);
            self.pending_brain_updates.run(self, brain);
            let tick_count = entity.tick_count.load(Ordering::Relaxed);
            if (tick_count + entity.entity_id) % DARKNESS_INTERVAL == 0 {
                Self::apply_darkness_around(&world, entity.pos.load(), Some(self), DARKNESS_RADIUS);
            }
            if tick_count % ANGERMANAGEMENT_TICK_DELAY == 0 {
                self.anger()
                    .tick(&world, |suspect| self.can_target_entity(suspect));
                self.sync_client_anger_level(brain);
            }
            ai::update_activity(brain);
            self.after_brain_access(brain);
        });
    }

    fn for_each_game_event_listener(
        &self,
        visit: &mut dyn FnMut(&dyn crate::world::game_event::GameEventListener),
    ) {
        visit(&vibration::WardenVibrationListener(self));
    }

    fn tick_links(&self) -> Vec<i32> {
        self.anger().suspect_ids().collect()
    }

    // Saved suspects are looked up anywhere, and a vibration's source and projectile owner are
    // looked up by UUID when it arrives.
    fn has_global_tick_reach(&self) -> bool {
        self.anger().has_unresolved_suspects()
            || self
                .vibration
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .may_arrive_next_tick()
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_compound("anger", self.anger().to_nbt());
        nbt.put_compound(
            "listener",
            self.vibration
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .write_nbt(),
        );
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        *self.anger() = nbt
            .get_compound("anger")
            .map(AngerManagement::from_nbt)
            .unwrap_or_default();
        self.with_own_brain(|warden, brain| warden.sync_client_anger_level(brain));
        *self
            .vibration
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = nbt
            .get_compound("listener")
            .map(VibrationData::read_nbt)
            .unwrap_or_default();
    }
}

impl crate::entity::custom_sound::CustomSound for WardenEntity {
    fn death_sound(&self) -> Option<Sound> {
        None
    }

    fn hurt_sound(&self) -> Option<Sound> {
        None
    }

    fn sound_volume(&self) -> f32 {
        4.0
    }
}
