use crate::entity::r#type::CreatureSpawnLevel;
use std::sync::{
    Arc, Weak,
    atomic::{AtomicI32, Ordering},
};

use pumpkin_data::BlockState;
use pumpkin_data::{
    damage::DamageType,
    effect::StatusEffect,
    entity::{EntityPose, EntityType},
    potion::Effect,
    sound::{Sound, SoundCategory},
    tracked_data,
};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::{GameMode, math::vector3::Vector3};

use crate::entity::{
    Entity, EntityBase,
    ai::goal::{
        Controls, Goal, look_around::RandomLookAroundGoal, look_at_entity::LookAtEntityGoal,
        melee_attack::MeleeAttackGoal, swim::SwimGoal, wander_around::WanderAroundGoal,
    },
    mob::{
        Mob, MobEntity,
        warden_anger::{AngerLevel, AngerManagement},
    },
};
use crate::plugin::api::events::entity::entity_target::TargetReason;
use crate::world::World;

const EMERGE_DURATION: i32 = 134;
const DARKNESS_DISPLAY_LIMIT: i32 = 200;
const DARKNESS_DURATION: i32 = 260;
const ANGERMANAGEMENT_TICK_DELAY: i32 = 20;
const ON_HURT_ANGER_BOOST: i32 = 20;

pub struct WardenEntity {
    pub mob_entity: MobEntity,
    emerge_ticks: Arc<AtomicI32>,
    anger_management: std::sync::Mutex<AngerManagement>,
}

struct EmergeGoal {
    emerge_ticks: Arc<AtomicI32>,
}

impl Goal for EmergeGoal {
    fn can_start(&mut self, _mob: &dyn Mob) -> bool {
        self.emerge_ticks.load(Ordering::Relaxed) > 0
    }

    fn start(&mut self, mob: &dyn Mob) {
        mob.get_mob_entity().steered().navigator().stop();
    }

    fn stop(&mut self, mob: &dyn Mob) {
        let entity = &mob.get_mob_entity().living_entity.entity;
        if entity.pose.load() == EntityPose::Emerging {
            entity.set_pose(EntityPose::Standing);
        }
    }

    fn tick(&mut self, _mob: &dyn Mob) {
        self.emerge_ticks.fetch_sub(1, Ordering::Relaxed);
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn can_stop(&self) -> bool {
        false
    }

    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK | Controls::JUMP | Controls::TARGET
    }
}

impl WardenEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let emerge_ticks = Arc::new(AtomicI32::new(0));
        let warden = Self {
            mob_entity,
            emerge_ticks: emerge_ticks.clone(),
            anger_management: std::sync::Mutex::new(AngerManagement::default()),
        };
        let mob_arc = Arc::new(warden);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        {
            let goal_selector = &mob_arc.mob_entity.goals_selector;

            goal_selector.add_goal(0, Box::new(EmergeGoal { emerge_ticks }));
            goal_selector.add_goal(0, Box::new(SwimGoal::default()));
            goal_selector.add_goal(4, Box::new(MeleeAttackGoal::new(1.0, true)));
            goal_selector.add_goal(5, Box::new(WanderAroundGoal::new(0.5)));
            goal_selector.add_goal(
                6,
                LookAtEntityGoal::with_default(mob_weak.clone(), &EntityType::PLAYER, 8.0),
            );
            goal_selector.add_goal(7, Box::new(RandomLookAroundGoal::default()));
        };

        mob_arc
    }

    pub fn emerge(&self) {
        let entity = &self.mob_entity.living_entity.entity;
        entity.set_pose(EntityPose::Emerging);
        self.emerge_ticks.store(EMERGE_DURATION, Ordering::Relaxed);
        entity.world.load().play_sound_fine(
            Sound::EntityWardenAgitated,
            SoundCategory::Hostile,
            &entity.pos.load(),
            5.0,
            1.0,
        );
    }

    fn is_digging_or_emerging(&self) -> bool {
        matches!(
            self.mob_entity.living_entity.entity.pose.load(),
            EntityPose::Digging | EntityPose::Emerging
        )
    }

    fn anger(&self) -> std::sync::MutexGuard<'_, AngerManagement> {
        self.anger_management
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
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
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains_box(&target.bounding_box.load())
    }

    pub fn get_anger_level(&self) -> AngerLevel {
        AngerLevel::by_anger(self.get_active_anger())
    }

    fn get_active_anger(&self) -> i32 {
        let target = self.mob_entity.get_target();
        self.anger().get_active_anger(target.as_deref())
    }

    pub fn get_entity_angry_at(&self) -> Option<Arc<dyn EntityBase>> {
        if !self.get_anger_level().is_angry() {
            return None;
        }
        self.anger()
            .get_active_entity(|entity| self.can_target_entity(entity))
    }

    pub fn increase_anger_at(&self, entity: &Arc<dyn EntityBase>, amount: i32, play_sound: bool) {
        if self.mob_entity.is_no_ai() || !self.can_target_entity(entity.as_ref()) {
            return;
        }
        let maybe_switch_target = self
            .mob_entity
            .get_target()
            .is_none_or(|target| target.get_player().is_none());
        let new_anger = self.anger().increase_anger(entity, amount);
        if entity.get_player().is_some()
            && maybe_switch_target
            && AngerLevel::by_anger(new_anger).is_angry()
        {
            self.set_mob_target(None, TargetReason::Angry);
        }
        if play_sound {
            self.play_listening_sound();
        }
    }

    fn play_listening_sound(&self) {
        if self.mob_entity.living_entity.entity.pose.load() != EntityPose::Roaring {
            self.play_entity_sound(
                self.get_anger_level().listening_sound(),
                10.0,
                self.mob_entity.living_entity.get_pitch(),
            );
        }
    }

    fn sync_client_anger_level(&self) {
        self.mob_entity.living_entity.entity.set_synced_data(
            tracked_data::warden::CLIENT_ANGER_LEVEL,
            VarInt(self.get_active_anger()),
        );
    }

    fn update_target_from_anger(&self) {
        let target = self.mob_entity.get_target();
        if let Some(target) = &target
            && (!self.get_anger_level().is_angry() || !self.can_target_entity(target.as_ref()))
        {
            if !self.can_target_entity(target.as_ref()) {
                self.anger().clear_anger(target.as_ref());
            }
            self.set_mob_target(None, TargetReason::ForgotTarget);
            return;
        }
        if target.is_none()
            && let Some(angry_at) = self.get_entity_angry_at()
        {
            self.set_mob_target(Some(angry_at), TargetReason::Angry);
        }
    }

    pub fn apply_darkness_around(world: &World, position: Vector3<f64>, radius: f64) {
        for player in world.get_nearby_players(position, radius) {
            if !matches!(
                player.gamemode.load(),
                GameMode::Survival | GameMode::Adventure
            ) {
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
            player.living_entity.add_effect(darkness);
        }
    }
}

impl Mob for WardenEntity {
    fn get_ambient_sound(&self) -> Option<Sound> {
        (self.mob_entity.living_entity.entity.pose.load() != EntityPose::Roaring
            && !self.is_digging_or_emerging())
        .then(|| self.get_anger_level().ambient_sound())
    }

    fn can_attack(&self, target: &dyn EntityBase) -> bool {
        self.can_target_entity(target)
    }

    fn remove_when_far_away(&self, _distance_sq: f64) -> bool {
        false
    }

    fn on_damage_with_context(
        &self,
        _damage_type: DamageType,
        source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> bool {
        if self.mob_entity.is_no_ai() || self.is_digging_or_emerging() {
            return true;
        }
        let Some(cause) = cause else {
            return true;
        };
        let world = self.mob_entity.living_entity.entity.world.load();
        let Some(attacker) = world.get_entity_by_id(cause.get_entity().entity_id) else {
            return true;
        };
        self.increase_anger_at(
            &attacker,
            AngerLevel::Angry.minimum_anger() + ON_HURT_ANGER_BOOST,
            false,
        );
        let direct = source
            .is_some_and(|source| source.get_entity().entity_id == cause.get_entity().entity_id);
        if self.mob_entity.get_target().is_none()
            && attacker.get_living_entity().is_some()
            && (direct
                || self
                    .get_entity()
                    .pos
                    .load()
                    .squared_distance_to_vec(&attacker.get_entity().pos.load())
                    < 5.0 * 5.0)
        {
            self.set_mob_target(Some(attacker), TargetReason::TargetAttackedEntity);
        }
        true
    }

    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        let entity = &self.mob_entity.living_entity.entity;
        if entity.tick_count.load(Ordering::Relaxed) % ANGERMANAGEMENT_TICK_DELAY == 0 {
            let world = entity.world.load();
            self.anger()
                .tick(&world, |suspect| self.can_target_entity(suspect));
            self.sync_client_anger_level();
        }
        self.update_target_from_anger();
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_compound("anger", self.anger().to_nbt());
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        *self.anger() = nbt
            .get_compound("anger")
            .map(AngerManagement::from_nbt)
            .unwrap_or_default();
        self.sync_client_anger_level();
    }

    fn as_custom_sound(&self) -> Option<&dyn crate::entity::custom_sound::CustomSound> {
        Some(self)
    }

    /// Vanilla `Warden.getWalkTargetValue`.
    fn get_walk_target_value(
        &self,
        _level: &dyn CreatureSpawnLevel,
        _pos: &pumpkin_util::math::position::BlockPos,
    ) -> f32 {
        0.0
    }

    fn mob_is_pushable(&self) -> bool {
        let pose = self.mob_entity.living_entity.entity.pose.load();
        pose != EntityPose::Digging
            && pose != EntityPose::Emerging
            && self.mob_entity.living_entity.is_pushable()
    }

    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {
        self.play_entity_sound(Sound::EntityWardenStep, 10.0, 1.0);
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn check_spawn_obstruction(&self, level: &dyn CreatureSpawnLevel) -> bool {
        let entity = &self.mob_entity.living_entity.entity;
        let bounding_box = entity.bounding_box.load();
        !level.contains_any_liquid(bounding_box)
            && level.entities_at_box(&bounding_box).is_empty()
            && level.is_space_empty(bounding_box)
    }
}

impl crate::entity::custom_sound::CustomSound for WardenEntity {
    fn death_sound(&self) -> Option<pumpkin_data::sound::Sound> {
        None
    }

    fn hurt_sound(&self) -> Option<pumpkin_data::sound::Sound> {
        None
    }

    fn sound_volume(&self) -> f32 {
        4.0
    }
}
