use crate::entity::r#type::CreatureSpawnLevel;
use pumpkin_util::random::gameplay::game_random;
use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, AtomicI32, Ordering},
};

use pumpkin_data::damage::DamageType;
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::potion::Effect;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::math::vector3::Vector3;

use crate::entity::{
    Entity, EntityBase,
    ageable::{AgeableData, AgeableMob},
    ai::goal::{
        Controls, Goal, active_target::ActiveTargetGoal, breed::BreedGoal,
        follow_parent::FollowParentGoal, look_around::RandomLookAroundGoal,
        look_at_entity::LookAtEntityGoal, melee_attack::MeleeAttackGoal, swim::SwimGoal,
        tempt::TemptGoal, try_find_water::TryFindWaterGoal, wander_around::WanderAroundGoal,
    },
    ai::target_match::TargetMatch,
    area_effect_cloud::INFINITE_DURATION,
    living::LivingEntity,
    mob::{Mob, MobEntity},
    passive::animal::Animal,
    player::Player,
};
use crate::world::World;

const TEMPT_ITEMS: &[&Item] = &[&Item::TROPICAL_FISH_BUCKET];

/// Vanilla `AxolotlAttackablesSensor.TARGET_DETECTION_DISTANCE`.
const TARGET_DETECTION_DISTANCE: f64 = 8.0;
/// How long vanilla `AxolotlAi.updateActivity` keeps `HAS_HUNTING_COOLDOWN` after a fight.
const HUNTING_COOLDOWN_TICKS: i32 = 2400;
/// The `PLAY_DEAD_TICKS` vanilla `Axolotl.hurtServer` sets.
const PLAY_DEAD_TICKS: i32 = 200;
/// Vanilla `Axolotl.onStopAttacking`: how far the killing player may be.
const SUPPORT_RANGE: f64 = 20.0;
/// Vanilla `Axolotl.applySupportingEffects`: the longest regeneration it tops up to.
const MAX_SUPPORT_REGENERATION: i32 = 2400;
const HUNTING_COOLDOWN_MEMORY: &str = "minecraft:has_hunting_cooldown";
const PLAY_DEAD_TICKS_MEMORY: &str = "minecraft:play_dead_ticks";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(i32)]
pub enum AxolotlVariant {
    #[default]
    Lucy = 0,
    Wild = 1,
    Gold = 2,
    Cyan = 3,
    Blue = 4,
}

impl AxolotlVariant {
    #[must_use]
    pub const fn from_id(id: i32) -> Self {
        match id {
            1 => Self::Wild,
            2 => Self::Gold,
            3 => Self::Cyan,
            4 => Self::Blue,
            _ => Self::Lucy,
        }
    }

    #[must_use]
    pub const fn id(self) -> i32 {
        self as i32
    }

    #[must_use]
    pub fn random_variant() -> Self {
        let rng = game_random();
        if rng.random_range(0..1200) == 0 {
            Self::Blue
        } else {
            match rng.random_range(0..4) {
                1 => Self::Wild,
                2 => Self::Gold,
                3 => Self::Cyan,
                _ => Self::Lucy,
            }
        }
    }
}

pub struct AxolotlEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    pub variant: AtomicI32,
    pub playing_dead: AtomicBool,
    pub from_bucket: AtomicBool,
    /// Vanilla's `PLAY_DEAD_TICKS` memory, 0 when absent.
    pub play_dead_ticks: AtomicI32,
    /// Ticks left on vanilla's `HAS_HUNTING_COOLDOWN` memory, 0 when absent.
    pub hunting_cooldown: AtomicI32,
}

impl AxolotlEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let variant = AxolotlVariant::random_variant();
        let axolotl = Self {
            mob_entity,
            ageable_data: AgeableData::default(),
            variant: AtomicI32::new(variant.id()),
            playing_dead: AtomicBool::new(false),
            from_bucket: AtomicBool::new(false),
            play_dead_ticks: AtomicI32::new(0),
            hunting_cooldown: AtomicI32::new(0),
        };
        let mob_arc = Arc::new(axolotl);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        {
            let goal_selector = &mob_arc.mob_entity.goals_selector;

            // Vanilla's `PLAY_DEAD` activity replaces every idle and fight behavior.
            goal_selector.add_goal(0, Box::new(PlayDeadGoal));
            goal_selector.add_goal(1, Box::new(TryFindWaterGoal));
            goal_selector.add_goal(1, Box::new(SwimGoal::default()));
            goal_selector.add_goal(2, BreedGoal::new(1.0));
            goal_selector.add_goal(3, Box::new(TemptGoal::new(1.25, TEMPT_ITEMS, false)));
            goal_selector.add_goal(4, Box::new(FollowParentGoal::new(1.25)));
            goal_selector.add_goal(5, Box::new(MeleeAttackGoal::new(1.2, false)));
            goal_selector.add_goal(6, Box::new(WanderAroundGoal::new(1.0)));
            goal_selector.add_goal(
                7,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 6.0),
            );
            goal_selector.add_goal(8, Box::new(RandomLookAroundGoal::default()));
        };

        mob_arc
            .mob_entity
            .target_selector
            .add_goal(1, Box::new(AxolotlHuntGoal::new(&mob_arc.mob_entity)));

        mob_arc
    }

    #[must_use]
    pub fn get_variant(&self) -> AxolotlVariant {
        AxolotlVariant::from_id(self.variant.load(Ordering::Relaxed))
    }

    pub fn set_variant(&self, variant: AxolotlVariant) {
        self.variant.store(variant.id(), Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::axolotl::DATA_VARIANT,
            VarInt(variant.id()),
        );
    }

    #[must_use]
    pub fn is_playing_dead(&self) -> bool {
        self.playing_dead.load(Ordering::Relaxed)
    }

    pub fn set_playing_dead(&self, playing_dead: bool) {
        self.playing_dead.store(playing_dead, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::axolotl::DATA_PLAYING_DEAD,
            playing_dead,
        );
    }

    #[must_use]
    pub fn is_from_bucket(&self) -> bool {
        self.from_bucket.load(Ordering::Relaxed)
    }

    /// Vanilla `Axolotl.onStopAttacking`.
    fn on_stop_attacking(&self, target: &dyn EntityBase) {
        let Some(living) = target.get_living_entity() else {
            return;
        };
        if !living.is_dead_or_dying() {
            return;
        }
        let Some((_, attacker_id)) = living.get_last_damage_source() else {
            return;
        };
        let world = self.get_entity().world.load();
        let Some(player) = world.get_player_by_id(attacker_id) else {
            return;
        };
        let area = self
            .get_entity()
            .bounding_box
            .load()
            .expand_all(SUPPORT_RANGE);
        if area.intersects(&player.get_entity().bounding_box.load()) {
            self.apply_supporting_effects(&player);
        }
    }

    /// Vanilla `Axolotl.applySupportingEffects`.
    pub fn apply_supporting_effects(&self, player: &Player) {
        let living = &player.living_entity;
        let regeneration = living.get_effect(&StatusEffect::REGENERATION);
        // Vanilla `MobEffectInstance.endsWithin(MAX_SUPPORT_REGENERATION - 1)`.
        if regeneration.as_ref().is_none_or(|effect| {
            effect.duration != INFINITE_DURATION && effect.duration < MAX_SUPPORT_REGENERATION
        }) {
            let previous = regeneration.map_or(0, |effect| effect.duration);
            living.add_effect_with_source(
                regeneration_effect((100 + previous).min(MAX_SUPPORT_REGENERATION)),
                Some(self),
            );
        }
        living.remove_effect(&StatusEffect::MINING_FATIGUE);
    }

    pub fn set_from_bucket(&self, from_bucket: bool) {
        self.from_bucket.store(from_bucket, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::axolotl::FROM_BUCKET,
            from_bucket,
        );
    }
}

const fn regeneration_effect(duration: i32) -> Effect {
    Effect {
        effect_type: &StatusEffect::REGENERATION,
        duration,
        amplifier: 0,
        ambient: false,
        show_particles: true,
        show_icon: true,
        blend: false,
    }
}

fn as_axolotl(mob: &dyn Mob) -> Option<&AxolotlEntity> {
    mob.cast_any().downcast_ref::<AxolotlEntity>()
}

/// Vanilla `AxolotlAttackablesSensor.isMatchingEntity`, without the attackable test the
/// target goal runs itself.
fn is_attackable_target(mob: &dyn Mob, target: &LivingEntity, _world: &World) -> bool {
    let Some(axolotl) = as_axolotl(mob) else {
        return false;
    };
    let entity_type = target.entity.entity_type;
    target
        .entity
        .pos
        .load()
        .squared_distance_to_vec(&axolotl.get_entity().pos.load())
        <= TARGET_DETECTION_DISTANCE * TARGET_DETECTION_DISTANCE
        && target.is_in_water()
        && (entity_type.has_tag(&tag::EntityType::MINECRAFT_AXOLOTL_ALWAYS_HOSTILES)
            || (axolotl.hunting_cooldown.load(Ordering::Relaxed) == 0
                && entity_type.has_tag(&tag::EntityType::MINECRAFT_AXOLOTL_HUNT_TARGETS)))
}

/// Vanilla `AxolotlAi`'s target handling: `StartAttacking` on the nearest attackable, and
/// when the fight ends `Axolotl.onStopAttacking` plus the hunting cooldown of
/// `AxolotlAi.updateActivity`.
struct AxolotlHuntGoal {
    inner: Box<ActiveTargetGoal>,
    target: Option<Arc<dyn EntityBase>>,
}

impl AxolotlHuntGoal {
    fn new(mob: &MobEntity) -> Self {
        Self {
            inner: ActiveTargetGoal::with_default(mob, TargetMatch::Any, true)
                .selecting(is_attackable_target),
            target: None,
        }
    }
}

impl Goal for AxolotlHuntGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        // `StartAttacking` belongs to the idle activity, which does not run while playing dead.
        !as_axolotl(mob).is_some_and(AxolotlEntity::is_playing_dead) && self.inner.can_start(mob)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.inner.should_continue(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.inner.start(mob);
        self.target = mob.get_mob_entity().get_target();
    }

    fn stop(&mut self, mob: &dyn Mob) {
        if let Some(axolotl) = as_axolotl(mob) {
            if let Some(target) = self.target.take() {
                axolotl.on_stop_attacking(target.as_ref());
            }
            axolotl
                .hunting_cooldown
                .store(HUNTING_COOLDOWN_TICKS, Ordering::Relaxed);
        }
        self.inner.stop(mob);
    }

    fn tick(&mut self, mob: &dyn Mob) {
        self.inner.tick(mob);
    }

    fn controls(&self) -> Controls {
        self.inner.controls()
    }
}

/// Vanilla `PlayDead`, running for the whole `PLAY_DEAD` activity so nothing else moves the
/// axolotl.
struct PlayDeadGoal;

impl Goal for PlayDeadGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        as_axolotl(mob).is_some_and(|axolotl| axolotl.play_dead_ticks.load(Ordering::Relaxed) > 0)
    }

    fn start(&mut self, mob: &dyn Mob) {
        let mob_entity = mob.get_mob_entity();
        if !mob_entity.living_entity.is_in_water() {
            return;
        }
        mob_entity.steered().navigator().stop();
        mob_entity
            .living_entity
            .add_effect(regeneration_effect(PLAY_DEAD_TICKS));
    }

    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK | Controls::JUMP
    }
}

impl AgeableMob for AxolotlEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }
}

impl Animal for AxolotlEntity {
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack.item.has_tag(&tag::Item::MINECRAFT_AXOLOTL_FOOD)
            || item_stack.item == &Item::TROPICAL_FISH_BUCKET
            || item_stack.item == &Item::TROPICAL_FISH
    }
}

impl Mob for AxolotlEntity {
    /// Vanilla `Axolotl.checkSpawnObstruction`.
    fn check_spawn_obstruction(&self, level: &dyn CreatureSpawnLevel) -> bool {
        crate::entity::mob::is_unobstructed(self, level)
    }

    /// Vanilla `Axolotl.getWalkTargetValue`.
    fn get_walk_target_value(
        &self,
        _level: &dyn CreatureSpawnLevel,
        _pos: &pumpkin_util::math::position::BlockPos,
    ) -> f32 {
        0.0
    }

    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn as_animal(&self) -> Option<&dyn Animal> {
        Some(self)
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_int("Variant", self.get_variant().id());
        nbt.put_bool("FromBucket", self.is_from_bucket());
        let mut memories = NbtCompound::new();
        let play_dead_ticks = self.play_dead_ticks.load(Ordering::Relaxed);
        if play_dead_ticks > 0 {
            let mut memory = NbtCompound::new();
            memory.put_int("value", play_dead_ticks);
            memories.put(PLAY_DEAD_TICKS_MEMORY, NbtTag::Compound(memory));
        }
        let hunting_cooldown = self.hunting_cooldown.load(Ordering::Relaxed);
        if hunting_cooldown > 0 {
            let mut memory = NbtCompound::new();
            memory.put_bool("value", true);
            memory.put_long("ttl", i64::from(hunting_cooldown));
            memories.put(HUNTING_COOLDOWN_MEMORY, NbtTag::Compound(memory));
        }
        if !memories.child_tags.is_empty() {
            let mut brain = NbtCompound::new();
            brain.put("memories", NbtTag::Compound(memories));
            nbt.put("Brain", NbtTag::Compound(brain));
        }
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        if let Some(variant) = nbt.get_int("Variant") {
            self.set_variant(AxolotlVariant::from_id(variant));
        }
        if let Some(from_bucket) = nbt.get_bool("FromBucket") {
            self.set_from_bucket(from_bucket);
        }
        if let Some(memories) = nbt
            .get_compound("Brain")
            .and_then(|brain| brain.get_compound("memories"))
        {
            if let Some(ticks) = memories
                .get_compound(PLAY_DEAD_TICKS_MEMORY)
                .and_then(|memory| memory.get_int("value"))
            {
                self.play_dead_ticks.store(ticks.max(0), Ordering::Relaxed);
            }
            if let Some(ttl) = memories
                .get_compound(HUNTING_COOLDOWN_MEMORY)
                .and_then(|memory| memory.get_long("ttl"))
            {
                self.hunting_cooldown
                    .store(ttl.clamp(0, i64::from(i32::MAX)) as i32, Ordering::Relaxed);
            }
        }
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        // The brain's memory expiry, then vanilla `ValidatePlayDead`.
        let _ = self
            .hunting_cooldown
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |ticks| {
                (ticks > 0).then(|| ticks - 1)
            });
        let play_dead_ticks = self.play_dead_ticks.load(Ordering::Relaxed);
        if play_dead_ticks > 0 {
            self.play_dead_ticks
                .store(play_dead_ticks - 1, Ordering::Relaxed);
        }
        if !self.mob_entity.is_no_ai() {
            let playing_dead = self.play_dead_ticks.load(Ordering::Relaxed) > 0;
            if playing_dead != self.is_playing_dead() {
                self.set_playing_dead(playing_dead);
            }
        }
    }

    /// The play dead roll of vanilla `Axolotl.hurtServer`, before the shared damage path.
    fn mob_hurt_server(
        &self,
        _caller: &dyn EntityBase,
        amount: f32,
        _damage_type: DamageType,
        _position: Option<Vector3<f64>>,
        source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> Option<bool> {
        let living = &self.mob_entity.living_entity;
        let health = living.health.load();
        let random = self.get_random();
        if !self.mob_entity.is_no_ai()
            && random.random_range(0..3) == 0
            && ((random.random_range(0..3) as f32) < amount
                || health / living.get_max_health() < 0.5)
            && amount < health
            && living.is_in_water()
            && (source.is_some() || cause.is_some())
            && !self.is_playing_dead()
        {
            self.play_dead_ticks
                .store(PLAY_DEAD_TICKS, Ordering::Relaxed);
        }
        None
    }

    fn mob_can_be_seen_as_enemy(&self) -> bool {
        !self.is_playing_dead()
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        let is_baby = entity.age.load(Ordering::Relaxed) < 0;
        if is_baby {
            entity.set_synced_data(pumpkin_data::tracked_data::axolotl::DATA_BABY_ID, true);
        }
        entity.set_synced_data(
            pumpkin_data::tracked_data::axolotl::DATA_VARIANT,
            VarInt(self.get_variant().id()),
        );
        entity.set_synced_data(
            pumpkin_data::tracked_data::axolotl::DATA_PLAYING_DEAD,
            self.is_playing_dead(),
        );
        entity.set_synced_data(
            pumpkin_data::tracked_data::axolotl::FROM_BUCKET,
            self.is_from_bucket(),
        );
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        let item = item_stack.get_item();

        if item == &Item::WATER_BUCKET {
            let entity = self.get_entity();
            let world = entity.world.load();
            if let Some(server) = world.server.upgrade() {
                let mut event = crate::plugin::api::events::player::player_bucket_entity::PlayerBucketEntityEvent {
                    player: player.clone(),
                    entity_id: entity.entity_id,
                    bucket_item: "axolotl_bucket".to_string(),
                    cancelled: false,
                };
                server.plugin_manager.fire_blocking(&server, &mut event);
                if event.cancelled {
                    return false;
                }
            }
            item_stack.decrement_unless_creative(player.gamemode.load(), 1);
            let pos = entity.pos.load();
            world.play_sound(Sound::ItemBucketFillAxolotl, SoundCategory::Neutral, &pos);
            entity.remove();
            return true;
        }

        self.animal_interact(player, item_stack, Sound::EntityAxolotlIdleAir)
    }
}
