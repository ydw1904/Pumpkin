use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicI32, Ordering},
};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::block_properties::{CreakingHeartLikeProperties, CreakingHeartState};
use pumpkin_data::damage::DamageType;
use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::entity::EntityStatus;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::particle::Particle;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::{Block, BlockState};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::bedrock::server::actor_event::ActorEventID;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::ser::NetworkWriteExt;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

use crate::block::entities::creaking_heart::CreakingHeartBlockEntity;
use crate::entity::ai::brain::memory::codec::{block_pos_from_tag, block_pos_to_tag};
use crate::entity::ai::brain::{Brain, memory::PackedMemories, memory::types};
use crate::entity::ai::pathfinder::{GroundPathNavigation, Navigator, node::PathType};
use crate::entity::custom_sound::CustomSound;
use crate::entity::r#type::CreatureSpawnLevel;
use crate::entity::{
    Entity, EntityBase,
    mob::{Mob, MobEntity},
    player::Player,
};
use crate::world::game_event::GameEventContext;
use crate::world::{World, entity_islands::HurtReach};

mod ai;

const MAX_HEALTH: f32 = 1.0;
const ATTACK_DAMAGE: f32 = 3.0;
const FOLLOW_RANGE: f32 = 32.0;
const ACTIVATION_RANGE_SQ: f64 = 144.0;
const MOVEMENT_SPEED_WHEN_FIGHTING: f32 = 0.4;
const SPEED_MULTIPLIER_WHEN_IDLING: f32 = 0.3;
const STEP_HEIGHT: f64 = 1.0625;
pub const CREAKING_ORANGE: i32 = 16_545_810;
pub const CREAKING_GRAY: i32 = 6_250_335;
const INVULNERABILITY_ANIMATION_DURATION: i32 = 8;
const TWITCH_DEATH_DURATION: u8 = 45;
const MAX_PLAYER_STUCK_COUNTER: i32 = 4;
/// Vanilla `isLookingAtMe(player, 0.5, ...)`.
const GAZE_CONE_SIZE: f64 = 0.5;
/// Vanilla `LivingEntity.hasLineOfSight` gives up past this distance.
const MAX_LINE_OF_SIGHT_DISTANCE: f64 = 128.0;
/// How far `CreakingHeartBlockEntity.spreadResin` can place resin from the heart: two logs,
/// then the face next to the second.
const RESIN_SPREAD_REACH: f64 = 3.0;

pub struct CreakingEntity {
    pub mob_entity: MobEntity,
    can_move: AtomicBool,
    is_active: AtomicBool,
    is_tearing_down: AtomicBool,
    /// Shared with the navigation's `HomeNodeEvaluator`.
    home_pos: Arc<AtomicCell<Option<BlockPos>>>,
    invulnerability_animation_remaining_ticks: AtomicI32,
    player_stuck_counter: AtomicI32,
}

impl CreakingEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        // Vanilla `Creaking.defineSynchedData`.
        entity
            .synched_data
            .define(pumpkin_data::tracked_data::creaking::CAN_MOVE, true);
        entity
            .synched_data
            .define(pumpkin_data::tracked_data::creaking::IS_ACTIVE, false);
        entity
            .synched_data
            .define(pumpkin_data::tracked_data::creaking::IS_TEARING_DOWN, false);
        entity.synched_data.define(
            pumpkin_data::tracked_data::creaking::HOME_POS,
            None::<BlockPos>,
        );
        let mut mob_entity = MobEntity::new(entity);
        let home_pos = Arc::new(AtomicCell::new(None));
        let mut navigator = Navigator::new(GroundPathNavigation::creaking(home_pos.clone()));
        navigator.set_can_float(true);
        *mob_entity
            .navigator
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = navigator;
        let creaking = Self {
            mob_entity,
            can_move: AtomicBool::new(true),
            is_active: AtomicBool::new(false),
            is_tearing_down: AtomicBool::new(false),
            home_pos,
            invulnerability_animation_remaining_ticks: AtomicI32::new(0),
            player_stuck_counter: AtomicI32::new(0),
        };

        // Vanilla `Creaking.createAttributes`.
        {
            let mut attributes = creaking
                .mob_entity
                .living_entity
                .attributes
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);

            for (attribute, value) in [
                (&Attributes::MAX_HEALTH, f64::from(MAX_HEALTH)),
                (
                    &Attributes::MOVEMENT_SPEED,
                    f64::from(MOVEMENT_SPEED_WHEN_FIGHTING),
                ),
                (&Attributes::ATTACK_DAMAGE, f64::from(ATTACK_DAMAGE)),
                (&Attributes::FOLLOW_RANGE, f64::from(FOLLOW_RANGE)),
                (&Attributes::STEP_HEIGHT, STEP_HEIGHT),
            ] {
                if let Some(instance) = attributes.get_mut(&attribute.id) {
                    instance.base_value = value;
                    instance.dirty.store(true, Ordering::Relaxed);
                }
            }
        }
        creaking.mob_entity.living_entity.health.store(MAX_HEALTH);

        Arc::new(creaking)
    }

    /// Vanilla `setTransient`: binds the creaking to the heart at `pos`.
    pub fn set_transient(&self, pos: BlockPos) {
        self.home_pos.store(Some(pos));
        self.mob_entity
            .living_entity
            .entity
            .set_synced_data(pumpkin_data::tracked_data::creaking::HOME_POS, Some(pos));
        // Vanilla `fireImmune` is true while heart-bound.
        self.mob_entity
            .living_entity
            .entity
            .fire_immune
            .store(true, Ordering::Relaxed);
        let mut navigator = self
            .mob_entity
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        navigator.set_pathfinding_malus(PathType::DamageOther, 8.0);
        navigator.set_pathfinding_malus(PathType::PowderSnow, 8.0);
        navigator.set_pathfinding_malus(PathType::Lava, 8.0);
        navigator.set_pathfinding_malus(PathType::DamageFire, 0.0);
        navigator.set_pathfinding_malus(PathType::DangerFire, 0.0);
    }

    pub fn get_home_pos(&self) -> Option<BlockPos> {
        self.home_pos.load()
    }

    pub fn is_heart_bound(&self) -> bool {
        self.get_home_pos().is_some()
    }

    pub fn can_move(&self) -> bool {
        self.can_move.load(Ordering::Relaxed)
    }

    fn set_can_move(&self, can_move: bool) {
        self.can_move.store(can_move, Ordering::Relaxed);
        self.mob_entity
            .living_entity
            .entity
            .set_synced_data(pumpkin_data::tracked_data::creaking::CAN_MOVE, can_move);
    }

    pub fn is_active(&self) -> bool {
        self.is_active.load(Ordering::Relaxed)
    }

    fn set_is_active(&self, active: bool) {
        self.is_active.store(active, Ordering::Relaxed);
        self.mob_entity
            .living_entity
            .entity
            .set_synced_data(pumpkin_data::tracked_data::creaking::IS_ACTIVE, active);
    }

    pub fn is_tearing_down(&self) -> bool {
        self.is_tearing_down.load(Ordering::Relaxed)
    }

    pub fn set_tearing_down(&self) {
        self.is_tearing_down.store(true, Ordering::Relaxed);
        self.mob_entity
            .living_entity
            .entity
            .set_synced_data(pumpkin_data::tracked_data::creaking::IS_TEARING_DOWN, true);
    }

    fn game_event(&self, event: GameEvent) {
        let entity = self.get_entity();
        entity.world.load().emit_game_event_with_context(
            event.name(),
            entity.pos.load(),
            &GameEventContext {
                source_entity: Some(self),
            },
        );
    }

    /// Vanilla `activate`.
    fn activate(&self, player: &Arc<Player>) {
        let target: Arc<dyn EntityBase> = player.clone();
        self.mob_entity
            .with_brain(self, |brain| brain.set(types::ATTACK_TARGET, target));
        self.game_event(GameEvent::EntityAction);
        self.make_sound(Sound::EntityCreakingActivate);
        self.set_is_active(true);
    }

    /// Vanilla `deactivate`.
    fn deactivate(&self) {
        self.mob_entity
            .with_brain(self, |brain| brain.erase(types::ATTACK_TARGET.id()));
        self.game_event(GameEvent::EntityAction);
        self.make_sound(Sound::EntityCreakingDeactivate);
        self.set_is_active(false);
    }

    fn nearest_players(&self) -> Vec<Arc<Player>> {
        self.mob_entity.with_brain(self, |brain| {
            brain
                .get(types::NEAREST_PLAYERS)
                .cloned()
                .unwrap_or_default()
        })
    }

    /// Vanilla `playerIsStuckInYou`: a player's eyes stayed inside the creaking for a while.
    pub fn player_is_stuck_in_you(&self) -> bool {
        let players = self.nearest_players();
        if players.is_empty() {
            self.player_stuck_counter.store(0, Ordering::Relaxed);
            return false;
        }
        let own_box = self.get_entity().bounding_box.load();
        for player in &players {
            let eye = player.get_entity().get_eye_pos();
            // Vanilla `AABB.contains`: min inclusive, max exclusive.
            if eye.x >= own_box.min.x
                && eye.x < own_box.max.x
                && eye.y >= own_box.min.y
                && eye.y < own_box.max.y
                && eye.z >= own_box.min.z
                && eye.z < own_box.max.z
            {
                let count = self.player_stuck_counter.fetch_add(1, Ordering::Relaxed) + 1;
                return count > MAX_PLAYER_STUCK_COUNTER;
            }
        }
        self.player_stuck_counter.store(0, Ordering::Relaxed);
        false
    }

    /// Vanilla `LivingEntity.isLookingAtMe(player, 0.5, false, true, gazeHeights)`: a fixed
    /// cone, then line of sight through the visual shapes, which see through glass.
    fn is_looking_at_me(&self, player: &Player, gaze_heights: &[f64]) -> bool {
        let creaking_pos = self.get_entity().pos.load();
        let player_entity = player.get_entity();
        let player_pos = player_entity.pos.load();
        let player_eye_y = player_entity.get_eye_y();
        let pitch = player_entity.pitch.load().to_radians();
        let yaw = -player_entity.yaw.load().to_radians();
        let cos_pitch = pitch.cos();
        let look = Vector3::new(
            f64::from(yaw.sin() * cos_pitch),
            f64::from(-pitch.sin()),
            f64::from(yaw.cos() * cos_pitch),
        )
        .normalize();
        let world = player_entity.world.load();
        let from = Vector3::new(player_pos.x, player_eye_y, player_pos.z);
        gaze_heights.iter().any(|&gaze_height| {
            let dir = Vector3::new(
                creaking_pos.x - player_pos.x,
                gaze_height - player_eye_y,
                creaking_pos.z - player_pos.z,
            )
            .normalize();
            let to = Vector3::new(creaking_pos.x, gaze_height, creaking_pos.z);
            look.dot(&dir) > 1.0 - GAZE_CONE_SIZE
                && (to - from).length() <= MAX_LINE_OF_SIGHT_DISTANCE
                && world
                    .clip(&pumpkin_world::clip::ClipContext::new(
                        from,
                        to,
                        pumpkin_world::clip::ClipBlock::Visual,
                        pumpkin_world::clip::ClipFluid::None,
                    ))
                    .is_miss
        })
    }

    /// Vanilla `LivingEntity.PLAYER_NOT_WEARING_DISGUISE_ITEM`, negated.
    fn is_player_disguised(player: &Player) -> bool {
        let equipment = player
            .living_entity
            .entity_equipment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        equipment
            .equipment
            .get(&EquipmentSlot::HEAD)
            .is_some_and(|head| {
                !head.is_empty()
                    && head
                        .item
                        .has_tag(&tag::Item::MINECRAFT_GAZE_DISGUISE_EQUIPMENT)
            })
    }

    /// Vanilla `checkCanMove`: frozen while a player that could be attacked looks at it, and
    /// activated by the first look from within 12 blocks.
    fn check_can_move(&self) -> bool {
        let players = self.nearest_players();
        let active = self.is_active();
        if players.is_empty() {
            if active {
                self.deactivate();
            }
            return true;
        }

        let entity = self.get_entity();
        let pos = entity.pos.load();
        let eye_y = entity.get_eye_y();
        let scale = self
            .mob_entity
            .living_entity
            .get_attribute_value(&Attributes::SCALE);
        let gaze_heights = [eye_y, pos.y + 0.5 * scale, f64::midpoint(eye_y, pos.y)];
        let mut has_potential_target = false;
        for player in &players {
            if !self.can_attack(player.as_ref())
                || (self as &dyn EntityBase).is_allied_to(player.as_ref())
            {
                continue;
            }
            has_potential_target = true;
            if (!active || !Self::is_player_disguised(player))
                && self.is_looking_at_me(player, &gaze_heights)
            {
                if active {
                    return false;
                }
                if player.get_entity().pos.load().squared_distance_to_vec(&pos)
                    < ACTIVATION_RANGE_SQ
                {
                    self.activate(player);
                    return false;
                }
            }
        }

        if !has_potential_target && active {
            self.deactivate();
        }
        true
    }

    /// Vanilla `tick`: whether the heart at its home still counts it as its protector.
    fn has_protection_from_creaking_heart(&self, world: &World, home_pos: &BlockPos) -> bool {
        world
            .get_block_entity(home_pos)
            .is_some_and(|block_entity| {
                block_entity
                    .as_any()
                    .downcast_ref::<CreakingHeartBlockEntity>()
                    .is_some_and(|heart| heart.is_protector(world, self))
            })
    }

    /// Vanilla `tearDown`: crumbles away without dying.
    pub fn tear_down(&self) {
        let entity = self.get_entity();
        let world = entity.world.load();
        let bounding_box = entity.bounding_box.load();
        let center = bounding_box.get_center();
        let size = bounding_box.max - bounding_box.min;
        let spread = Vector3::new(
            (size.x * 0.3) as f32,
            (size.y * 0.3) as f32,
            (size.z * 0.3) as f32,
        );
        let mut heart = CreakingHeartLikeProperties::default(&Block::CREAKING_HEART);
        heart.creaking_heart_state = CreakingHeartState::Awake;
        for (state_id, count) in [
            (Block::PALE_OAK_WOOD.default_state.id, 100),
            (heart.to_state_id(&Block::CREAKING_HEART), 10),
        ] {
            let mut data = Vec::new();
            if data
                .write_var_int(&VarInt(i32::from(state_id.as_u16())))
                .is_ok()
            {
                world.send_particles_with_data(
                    Particle::BlockCrumble,
                    &data,
                    false,
                    false,
                    center,
                    count,
                    spread,
                    0.0,
                );
            }
        }
        self.make_sound(Sound::EntityCreakingDeath);
        entity.remove();
    }

    /// Vanilla `creakingDeathEffects`: dies by `killer`'s hand and twitches.
    pub fn creaking_death_effects(&self, killer: &Arc<Player>) {
        let living = &self.mob_entity.living_entity;
        let attacker: &dyn EntityBase = killer.as_ref();
        living.resolve_mob_responsible_for_damage(DamageType::PLAYER_ATTACK, Some(attacker));
        living.resolve_player_responsible_for_damage(Some(attacker));
        let mut death_event =
            crate::plugin::api::events::entity::entity_death::EntityDeathEvent::new(
                self.get_entity().entity_id,
                0,
            );
        if let Some(server) = self.get_entity().world.load().server.upgrade() {
            server
                .plugin_manager
                .fire_blocking(&server, &mut death_event);
        }
        // `die` without the death sound, which plays when it finishes tearing down.
        living.die(
            DamageType::PLAYER_ATTACK,
            Some(attacker),
            Some(attacker),
            false,
        );
        self.make_sound(Sound::EntityCreakingTwitch);
    }

    /// The area `CreakingHeartBlockEntity.creakingHurt` can reach around the heart.
    fn heart_reach(home_pos: &BlockPos) -> pumpkin_util::math::boundingbox::BoundingBox {
        pumpkin_util::math::boundingbox::BoundingBox::from_block(home_pos).expand(
            RESIN_SPREAD_REACH,
            RESIN_SPREAD_REACH,
            RESIN_SPREAD_REACH,
        )
    }
}

impl CustomSound for CreakingEntity {
    fn death_sound(&self) -> Option<Sound> {
        None
    }

    fn hurt_sound(&self) -> Option<Sound> {
        self.is_heart_bound().then_some(Sound::EntityCreakingSway)
    }
}

impl Mob for CreakingEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn make_brain(&self, packed: &PackedMemories) -> Brain {
        ai::make_brain(self, packed)
    }

    fn as_custom_sound(&self) -> Option<&dyn CustomSound> {
        Some(self)
    }

    fn get_ambient_sound(&self) -> Option<Sound> {
        (!self.is_active()).then_some(Sound::EntityCreakingAmbient)
    }

    /// Vanilla `Creaking.getWalkTargetValue`.
    fn get_walk_target_value(&self, _level: &dyn CreatureSpawnLevel, _pos: &BlockPos) -> f32 {
        0.0
    }

    fn mob_is_pushable(&self) -> bool {
        self.mob_entity.living_entity.is_pushable() && self.can_move()
    }

    fn mob_knockback(
        &self,
        _power: f64,
        _x: f64,
        _z: f64,
        _source: Option<&Entity>,
        _damage: f32,
        _comes_from_effect: bool,
    ) -> bool {
        // Vanilla `knockback` does nothing while frozen.
        !self.can_move()
    }

    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {
        self.play_entity_sound(Sound::EntityCreakingStep, 0.15, 1.0);
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        if let Some(pos) = self.get_home_pos() {
            nbt.put("home_pos", block_pos_to_tag(pos));
        }
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        if let Some(pos) = nbt.get("home_pos").and_then(block_pos_from_tag) {
            self.set_transient(pos);
        }
    }

    /// Vanilla `Creaking.tick` before `super.tick`, and its `tickDeath` override.
    fn mob_tick(&self, _caller: &dyn EntityBase) {
        let entity = self.get_entity();
        let world = entity.world.load();
        let living = &self.mob_entity.living_entity;
        if let Some(home_pos) = self.get_home_pos()
            && !self.has_protection_from_creaking_heart(&world, &home_pos)
        {
            living.set_health(0.0);
        }

        if self.ticks_own_death() && living.health.load() <= 0.0 {
            let death_time = living.death_time.fetch_add(1, Ordering::Relaxed) + 1;
            if death_time > TWITCH_DEATH_DURATION && !entity.is_removed() {
                self.tear_down();
            }
        }
    }

    fn ticks_own_death(&self) -> bool {
        self.is_heart_bound() && self.is_tearing_down()
    }

    /// Vanilla `Creaking.aiStep` before `super.aiStep`.
    fn before_mob_ai_step(&self) {
        if self
            .invulnerability_animation_remaining_ticks
            .load(Ordering::Relaxed)
            > 0
        {
            self.invulnerability_animation_remaining_ticks
                .fetch_sub(1, Ordering::Relaxed);
        }

        let can_move = self.can_move();
        let now_can_move = self.check_can_move();
        if now_can_move != can_move {
            self.game_event(GameEvent::EntityAction);
            if now_can_move {
                self.make_sound(Sound::EntityCreakingUnfreeze);
            } else {
                self.mob_entity.stop_in_place();
                self.make_sound(Sound::EntityCreakingFreeze);
            }
        }
        self.set_can_move(now_can_move);
    }

    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        self.mob_entity.tick_brain(self);
        let can_move = self.can_move();
        let attack_target = self.mob_entity.with_brain(self, |brain| {
            ai::update_activity(brain, can_move);
            brain.get(types::ATTACK_TARGET).cloned()
        });
        // `getTarget` reads the brain's attack target.
        self.mob_entity.set_target(attack_target);
    }

    // Vanilla `CreakingPathNavigation`, `CreakingMoveControl`, `CreakingLookControl`,
    // `CreakingJumpControl` and `CreakingBodyRotationControl` all pause while frozen.
    fn ticks_navigation(&self) -> bool {
        self.can_move()
    }

    fn ticks_move_control(&self) -> bool {
        self.can_move()
    }

    fn ticks_look_control(&self) -> bool {
        self.can_move()
    }

    fn ticks_body_rotation(&self) -> bool {
        self.can_move()
    }

    fn tick_jump_control(&self, jump: bool) {
        self.mob_entity
            .living_entity
            .jumping
            .store(jump && self.can_move(), Ordering::SeqCst);
    }

    /// Vanilla `Creaking.doHurtTarget`.
    fn do_hurt_target(&self, target: &dyn EntityBase) -> bool {
        if target.get_living_entity().is_none() {
            return false;
        }
        let entity = self.get_entity();
        entity.world.load().send_entity_status(
            entity,
            EntityStatus::StartAttacking,
            ActorEventID::None,
        );
        let damaged = self.mob_entity.try_attack(self, target);
        if damaged {
            self.on_attack(target);
            // Vanilla `Mob.doHurtTarget` ends with `playAttackSound`.
            self.make_sound(Sound::EntityCreakingAttack);
        }
        damaged
    }

    /// Vanilla `Creaking.hurtServer`: while bound to a heart, hits only twitch it and call
    /// the heart.
    fn mob_hurt_server(
        &self,
        _caller: &dyn EntityBase,
        _amount: f32,
        damage_type: DamageType,
        _position: Option<Vector3<f64>>,
        source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> Option<bool> {
        let home_pos = self.get_home_pos()?;
        if damage_type.has_tag(&tag::DamageType::MINECRAFT_BYPASSES_INVULNERABILITY) {
            return None;
        }
        let entity = self.get_entity();
        let living = &self.mob_entity.living_entity;
        let attacker = cause.or(source);
        if entity.is_invulnerable_to(&damage_type, attacker)
            || self
                .invulnerability_animation_remaining_ticks
                .load(Ordering::Relaxed)
                > 0
            || living.is_dead_or_dying()
        {
            return Some(false);
        }
        living.resolve_mob_responsible_for_damage(damage_type, attacker);
        let responsible_player = living.resolve_player_responsible_for_damage(attacker);
        let direct_is_living_or_projectile = source.is_some_and(|direct| {
            direct.get_living_entity().is_some() || direct.get_projectile_owner().is_some()
        });
        if !direct_is_living_or_projectile && responsible_player.is_none() {
            return Some(false);
        }
        self.invulnerability_animation_remaining_ticks
            .store(INVULNERABILITY_ANIMATION_DURATION, Ordering::Relaxed);
        let world = entity.world.load_full();
        world.send_entity_status(entity, EntityStatus::Shake, ActorEventID::None);
        self.game_event(GameEvent::EntityAction);
        if let Some(block_entity) = world.get_block_entity(&home_pos)
            && let Some(heart) = block_entity
                .as_any()
                .downcast_ref::<CreakingHeartBlockEntity>()
            && heart.is_protector(&world, self)
        {
            if responsible_player.is_some() {
                heart.creaking_hurt(&world);
            }
            // Vanilla `Mob.playHurtSound`.
            self.reset_ambient_sound_time();
            self.make_sound(Sound::EntityCreakingSway);
        }
        Some(true)
    }

    // A hit calls its heart, which spreads resin on the logs around it.
    fn hurt_reach(&self) -> Option<HurtReach> {
        let home_pos = self.get_home_pos()?;
        Some(HurtReach {
            area: self
                .get_entity()
                .bounding_box
                .load()
                .union(&Self::heart_reach(&home_pos)),
            changes_blocks: true,
        })
    }
}
