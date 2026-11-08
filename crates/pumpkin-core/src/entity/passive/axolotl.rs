use crate::entity::r#type::CreatureSpawnLevel;
use pumpkin_util::random::gameplay::GameRandom;
use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, AtomicI32, Ordering},
};

use pumpkin_data::Block;
use pumpkin_data::damage::DamageType;
use pumpkin_data::data_component_impl::{AxolotlVariantImpl, BucketEntityDataImpl};
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::potion::Effect;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

use crate::entity::{
    Entity, EntityBase,
    ageable::{AgeableData, AgeableMob, BABY_START_AGE, DEFAULT_BABY_SPAWN_CHANCE},
    ai::brain::behavior::random_stroll,
    ai::control::{
        Control, LookControlTrait, MoveControlTrait,
        smooth_swimming_look_control::SmoothSwimmingLookControl,
        smooth_swimming_move_control::SmoothSwimmingMoveControl,
    },
    ai::goal::{
        Controls, Goal, GoalReach, active_target::ActiveTargetGoal, breed::BreedGoal,
        follow_parent::FollowParentGoal, look_around::RandomLookAroundGoal,
        look_at_entity::LookAtEntityGoal, melee_attack::MeleeAttackGoal, tempt::TemptGoal,
    },
    ai::pathfinder::{Navigator, NavigatorGoal, node::PathType},
    ai::target_match::TargetMatch,
    ai::util::land_random_pos,
    area_effect_cloud::INFINITE_DURATION,
    bucketable::{Bucketable, bucket_mob_pickup},
    living::LivingEntity,
    mob::{
        Mob, MobEntity,
        spawn::{EntitySpawnReason, SpawnGroupData},
    },
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
/// Vanilla `AxolotlAi` speed modifiers. Out of water every behavior but breeding moves at
/// `SPEED_MULTIPLIER_ON_LAND`, which `AxolotlMoveControl` applies.
const SPEED_MULTIPLIER_WHEN_MAKING_LOVE: f64 = 0.2;
const SPEED_MULTIPLIER_ON_LAND: f64 = 0.15;
const SPEED_MULTIPLIER_WHEN_IDLING_IN_WATER: f64 = 0.5;
const SPEED_MULTIPLIER_WHEN_CHASING_IN_WATER: f64 = 0.6;
const SPEED_MULTIPLIER_WHEN_FOLLOWING_ADULT_IN_WATER: f64 = 0.6;
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

    /// Vanilla `Variant.getCommonSpawnVariant`.
    #[must_use]
    pub fn common_spawn_variant(random: GameRandom) -> Self {
        Self::from_id(random.random_range(0..4))
    }

    /// Vanilla `Variant.getRareSpawnVariant`: blue is the only rare one.
    #[must_use]
    pub const fn rare_spawn_variant() -> Self {
        Self::Blue
    }
}

/// Vanilla `Axolotl.useRareVariant`: one in this many bred babies is blue.
const RARE_VARIANT_CHANCE: i32 = 1200;
/// Vanilla `Axolotl.AXOLOTL_TOTAL_AIR_SUPPLY`.
const AXOLOTL_TOTAL_AIR_SUPPLY: i32 = 6000;
/// Vanilla `Axolotl.rehydrate`: air a water bottle splash gives back.
const REHYDRATE_AIR_SUPPLY: i32 = 1800;
/// Vanilla `Axolotl.handleAirSupply`: damage taken each time it dries out.
const DRY_OUT_DAMAGE: f32 = 2.0;

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
        entity
            .air_supply
            .store(AXOLOTL_TOTAL_AIR_SUPPLY, Ordering::Relaxed);
        // Vanilla `Entity.defineSynchedData` defines the air supply as `getMaxAirSupply()`.
        entity.synched_data.define(
            pumpkin_data::tracked_data::axolotl::DATA_AIR_SUPPLY_ID,
            VarInt(AXOLOTL_TOTAL_AIR_SUPPLY),
        );
        // Vanilla `Axolotl.defineSynchedData`.
        entity.synched_data.define(
            pumpkin_data::tracked_data::axolotl::DATA_VARIANT,
            VarInt(pumpkin_data::entity_variant::AxolotlVariant::DEFAULT.id()),
        );
        entity.synched_data.define(
            pumpkin_data::tracked_data::axolotl::DATA_PLAYING_DEAD,
            false,
        );
        entity
            .synched_data
            .define(pumpkin_data::tracked_data::axolotl::FROM_BUCKET, false);
        let mut mob_entity = MobEntity::new(entity);
        // Vanilla `Axolotl` constructor and `createNavigation`.
        *mob_entity
            .move_control
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Box::new(AxolotlMoveControl(
            SmoothSwimmingMoveControl::new(85, 10, 0.1, 0.5, false),
        ));
        *mob_entity
            .look_control
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Box::new(AxolotlLookControl(SmoothSwimmingLookControl::new(20)));
        let mut navigator = Navigator::amphibious(false);
        navigator.set_pathfinding_malus(PathType::Water, 0.0);
        *mob_entity
            .navigator
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = navigator;
        let axolotl = Self {
            mob_entity,
            ageable_data: AgeableData::default(),
            variant: AtomicI32::new(AxolotlVariant::default().id()),
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
            goal_selector.add_goal(2, BreedGoal::new(SPEED_MULTIPLIER_WHEN_MAKING_LOVE));
            goal_selector.add_goal(
                3,
                Box::new(TemptGoal::new(
                    SPEED_MULTIPLIER_WHEN_IDLING_IN_WATER,
                    TEMPT_ITEMS,
                    false,
                )),
            );
            goal_selector.add_goal(
                4,
                Box::new(FollowParentGoal::new(
                    SPEED_MULTIPLIER_WHEN_FOLLOWING_ADULT_IN_WATER,
                )),
            );
            goal_selector.add_goal(5, Box::new(TryFindLiquidGoal::default()));
            goal_selector.add_goal(
                5,
                Box::new(MeleeAttackGoal::new(
                    SPEED_MULTIPLIER_WHEN_CHASING_IN_WATER,
                    false,
                )),
            );
            goal_selector.add_goal(6, Box::new(AxolotlStrollGoal::default()));
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

    /// Vanilla `Axolotl.rehydrate`, from a water bottle splash.
    pub fn rehydrate(&self) {
        let entity = self.get_entity();
        let air = entity.air_supply.load(Ordering::Relaxed) + REHYDRATE_AIR_SUPPLY;
        entity.set_air_supply(air.min(AXOLOTL_TOTAL_AIR_SUPPLY));
    }

    /// Vanilla `Axolotl.handleAirSupply`: dries out out of water and rain.
    fn handle_air_supply(&self, pre_tick_air_supply: i32) {
        let entity = self.get_entity();
        let living = &self.mob_entity.living_entity;
        if living.is_alive() && !entity.is_in_water_or_rain() {
            entity.set_air_supply(pre_tick_air_supply - 1);
            if living.should_take_drowning_damage() {
                entity.set_air_supply(0);
                self.damage(self, DRY_OUT_DAMAGE, DamageType::DRY_OUT);
            }
        } else {
            entity.set_air_supply(AXOLOTL_TOTAL_AIR_SUPPLY);
        }
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
    fn tick_reach(&self, mob: &dyn Mob, running: bool) -> Option<GoalReach> {
        let mut reach = self.inner.tick_reach(mob, running)?;
        // Stopping supports the player whose damage was last on the old target. An attacker
        // that damages it earlier in the tick links its own owner.
        if running && let Some(target) = &self.target {
            reach.links.push(target.get_entity().entity_id);
            reach.links.extend(
                target
                    .get_living_entity()
                    .map(LivingEntity::last_damage_attacker_id)
                    .filter(|id| *id != 0),
            );
        }
        Some(reach)
    }

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

/// Vanilla `Axolotl.AxolotlMoveControl`: still while playing dead. Out of water it moves at
/// `AxolotlAi`'s land speed.
struct AxolotlMoveControl(SmoothSwimmingMoveControl);

impl Control for AxolotlMoveControl {}

impl MoveControlTrait for AxolotlMoveControl {
    fn tick(&mut self, mob: &dyn Mob) {
        if as_axolotl(mob).is_some_and(AxolotlEntity::is_playing_dead) {
            return;
        }
        if !mob.get_entity().is_in_water()
            && self.0.speed_modifier != SPEED_MULTIPLIER_WHEN_MAKING_LOVE
        {
            self.0.speed_modifier = SPEED_MULTIPLIER_ON_LAND;
        }
        self.0.tick(mob);
    }

    fn set_wanted_position(&mut self, x: f64, y: f64, z: f64, speed_modifier: f64) {
        self.0.set_wanted_position(x, y, z, speed_modifier);
    }

    fn strafe(&mut self, forward: f32, right: f32) {
        self.0.strafe(forward, right);
    }

    fn has_wanted(&self) -> bool {
        self.0.has_wanted()
    }
}

/// Vanilla `Axolotl.AxolotlLookControl`: still while playing dead.
struct AxolotlLookControl(SmoothSwimmingLookControl);

impl Control for AxolotlLookControl {}

impl LookControlTrait for AxolotlLookControl {
    fn tick(&mut self, mob: &dyn Mob) {
        if !as_axolotl(mob).is_some_and(AxolotlEntity::is_playing_dead) {
            self.0.tick(mob);
        }
    }

    fn look_at_with_range(
        &mut self,
        x: f64,
        y: f64,
        z: f64,
        max_yaw_change: f32,
        max_pitch_change: f32,
    ) {
        self.0
            .look_at_with_range(x, y, z, max_yaw_change, max_pitch_change);
    }
}

/// Vanilla `PlayDead`, running for the whole `PLAY_DEAD` activity so nothing else moves the
/// axolotl.
struct PlayDeadGoal;

impl Goal for PlayDeadGoal {
    fn tick_reach(&self, _mob: &dyn Mob, _running: bool) -> Option<GoalReach> {
        Some(GoalReach::default())
    }

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

/// Vanilla `TryFindLiquid.create(6, 0.15F, FluidTags.AXOLOTL_TRIES_TO_FIND)`: on land, walk to
/// the nearest water within 6 blocks, preferring water open to the air.
#[derive(Default)]
struct TryFindLiquidGoal {
    next_ok_start_time: i64,
    target: Option<BlockPos>,
}

impl TryFindLiquidGoal {
    const RANGE: i32 = 6;

    fn find_liquid(mob: &dyn Mob) -> Option<BlockPos> {
        let entity = mob.get_entity();
        let world = entity.world.load();
        let body_pos = entity.block_pos.load();
        let position = entity.pos.load();
        let mut found = None;
        for pos in BlockPos::within_box_by_manhattan_distance(
            body_pos,
            Self::RANGE,
            Self::RANGE,
            Self::RANGE,
        ) {
            if pos.0.x == body_pos.0.x && pos.0.z == body_pos.0.z {
                continue;
            }
            let (block, state) = world.get_block_and_state(&pos);
            if !(block == &Block::WATER || block == &Block::LAVA)
                || !World::get_fluid_from_state_id(state.id)
                    .has_tag(&tag::Fluid::MINECRAFT_AXOLOTL_TRIES_TO_FIND)
            {
                continue;
            }
            if world.get_block_state(&pos.up()).is_air() {
                return Some(pos);
            }
            // Vanilla `closerToCenterThan(position, 1.5)`.
            if found.is_none()
                && pos.to_centered_f64().squared_distance_to_vec(&position) >= 1.5 * 1.5
            {
                found = Some(pos);
            }
        }
        found
    }
}

impl Goal for TryFindLiquidGoal {
    fn tick_reach(&self, _mob: &dyn Mob, _running: bool) -> Option<GoalReach> {
        // `find_liquid` reads blocks within the path search range.
        Some(GoalReach {
            speed: SPEED_MULTIPLIER_ON_LAND,
            ..GoalReach::default()
        })
    }

    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let entity = mob.get_entity();
        if mob.get_mob_entity().get_target().is_some()
            || !mob.is_navigator_idle()
            || entity
                .world
                .load()
                .get_fluid(&entity.block_pos.load())
                .has_tag(&tag::Fluid::MINECRAFT_AXOLOTL_TRIES_TO_FIND)
        {
            return false;
        }
        let now = entity
            .world
            .load()
            .level_time
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .world_age;
        if now < self.next_ok_start_time {
            self.next_ok_start_time = now + 22;
            return false;
        }
        self.target = Self::find_liquid(mob);
        self.next_ok_start_time = now + 40;
        self.target.is_some()
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        !mob.is_navigator_idle()
    }

    fn start(&mut self, mob: &dyn Mob) {
        if let Some(target) = self.target {
            let mob_entity = mob.get_mob_entity();
            let target = target.to_centered_f64();
            mob_entity
                .look_control
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .look_at_position(mob, target);
            let pos = mob.get_entity().pos.load();
            mob_entity
                .steered()
                .navigator()
                .set_progress(NavigatorGoal::new(pos, target, SPEED_MULTIPLIER_ON_LAND));
        }
    }

    fn tick(&mut self, mob: &dyn Mob) {
        if let Some(target) = self.target {
            mob.get_mob_entity()
                .look_control
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .look_at_position(mob, target.to_centered_f64());
        }
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.target = None;
        mob.get_mob_entity().steered().navigator().stop();
    }

    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK
    }
}

/// The strolls of vanilla `AxolotlAi`'s idle activity: `RandomStroll.swim` in water and
/// `RandomStroll.stroll` without leaving water on land, tried whenever there is no walk target.
#[derive(Default)]
struct AxolotlStrollGoal {
    target: Option<(Vector3<f64>, f64)>,
}

impl Goal for AxolotlStrollGoal {
    fn tick_reach(&self, _mob: &dyn Mob, _running: bool) -> Option<GoalReach> {
        Some(GoalReach {
            speed: SPEED_MULTIPLIER_WHEN_IDLING_IN_WATER,
            ..GoalReach::default()
        })
    }

    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        self.target = if mob.get_entity().is_in_water() {
            random_stroll::get_target_swim_pos(mob)
                .map(|pos| (pos, SPEED_MULTIPLIER_WHEN_IDLING_IN_WATER))
        } else {
            land_random_pos::get_pos(mob, 10, 7).map(|pos| (pos, SPEED_MULTIPLIER_ON_LAND))
        };
        self.target.is_some()
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        !mob.is_navigator_idle()
    }

    fn start(&mut self, mob: &dyn Mob) {
        if let Some((target, speed)) = self.target.take() {
            let pos = mob.get_entity().pos.load();
            mob.get_mob_entity()
                .steered()
                .navigator()
                .set_progress(NavigatorGoal::new(pos, target, speed));
        }
    }

    fn stop(&mut self, mob: &dyn Mob) {
        mob.get_mob_entity().steered().navigator().stop();
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
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

    /// Vanilla `Axolotl.getBreedOffspring`: a rare blue, else either parent's variant.
    fn get_breed_offspring(&self, partner: &dyn EntityBase) -> Arc<dyn EntityBase> {
        let entity = self.get_entity();
        let baby = Self::new(Entity::new(
            entity.world.load_full(),
            entity.pos.load(),
            &EntityType::AXOLOTL,
        ));
        let random = self.get_random();
        let variant = if random.random_range(0..RARE_VARIANT_CHANCE) == 0 {
            AxolotlVariant::rare_spawn_variant()
        } else if random.random::<bool>() {
            self.get_variant()
        } else {
            partner
                .cast_any()
                .downcast_ref::<Self>()
                .map_or_else(|| self.get_variant(), Self::get_variant)
        };
        baby.set_variant(variant);
        baby.mob_entity
            .persistence_required
            .store(true, Ordering::Relaxed);
        baby
    }
}

impl Mob for AxolotlEntity {
    fn get_ambient_sound(&self) -> Option<Sound> {
        Some(if self.get_entity().is_in_water() {
            Sound::EntityAxolotlIdleWater
        } else {
            Sound::EntityAxolotlIdleAir
        })
    }

    fn play_ambient_sound(&self) {
        if !self.is_playing_dead()
            && let Some(sound) = self.get_ambient_sound()
        {
            self.make_sound(sound);
        }
    }

    fn has_audited_tick(&self) -> bool {
        true
    }

    /// Vanilla `Axolotl.baseTick`: `handleAirSupply` after `LivingEntity.baseTick`'s air.
    fn base_tick(&self, pre_tick_air_supply: i32) {
        if !self.mob_entity.is_no_ai() {
            self.handle_air_supply(pre_tick_air_supply);
        }
    }

    fn mob_max_air_supply(&self) -> i32 {
        AXOLOTL_TOTAL_AIR_SUPPLY
    }

    /// Vanilla `Axolotl.finalizeSpawn`: a group shares two common variants, and the third
    /// axolotl on is a baby. Buckets keep the variant they carry.
    fn finalize_spawn(
        &self,
        _world: &Arc<World>,
        reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        if reason == EntitySpawnReason::Bucket {
            return group_data;
        }
        let random = self.get_random();
        let (variants, group_size) = match group_data {
            Some(SpawnGroupData::Axolotl {
                variants,
                group_size,
            }) => (variants, group_size),
            _ => (
                [
                    AxolotlVariant::common_spawn_variant(random),
                    AxolotlVariant::common_spawn_variant(random),
                ],
                0,
            ),
        };
        self.set_variant(variants[random.random_range(0..variants.len())]);
        if group_size >= 2 {
            self.set_age(BABY_START_AGE);
        }
        // `AxolotlGroupData` passes `shouldSpawnBaby = false` to `AgeableMobGroupData`.
        let group_size = match self.finalize_ageable_spawn(Some(SpawnGroupData::Ageable {
            group_size,
            should_spawn_baby: false,
            baby_spawn_chance: DEFAULT_BABY_SPAWN_CHANCE,
        })) {
            Some(SpawnGroupData::Ageable { group_size, .. }) => group_size,
            _ => group_size + 1,
        };
        Some(SpawnGroupData::Axolotl {
            variants,
            group_size,
        })
    }

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

    fn get_max_look_pitch_change(&self) -> f32 {
        1.0
    }

    fn get_max_head_rotation(&self) -> f32 {
        1.0
    }

    fn mob_is_pushed_by_fluids(&self) -> bool {
        false
    }

    /// Vanilla `Axolotl.travelInWater`: no gravity under water.
    fn travel_in_water(&self, caller: &dyn EntityBase, _gravity: f64, _falling: bool) {
        let living = &self.mob_entity.living_entity;
        let entity = &living.entity;
        entity.update_velocity_from_input(
            living.movement_input.load(),
            f64::from(living.get_speed()),
        );
        living.make_move(caller);
        entity.velocity.store(entity.velocity.load() * 0.9);
    }

    fn as_animal(&self) -> Option<&dyn Animal> {
        Some(self)
    }

    fn as_bucketable(&self) -> Option<&dyn Bucketable> {
        Some(self)
    }

    fn mob_set_variant_name(&self, name: &str) {
        if let Some(variant) = pumpkin_data::entity_variant::AxolotlVariant::from_name(name) {
            self.set_variant(AxolotlVariant::from_id(variant.id()));
        }
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

    fn mob_interact_with_hand(
        &self,
        player: &Arc<Player>,
        item_stack: &mut ItemStack,
        _hand: pumpkin_util::Hand,
    ) -> Option<bool> {
        bucket_mob_pickup(player, item_stack, self)
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        self.animal_interact(player, item_stack)
    }
}

impl Bucketable for AxolotlEntity {
    fn is_from_bucket(&self) -> bool {
        self.from_bucket.load(Ordering::Relaxed)
    }

    fn set_from_bucket(&self, from_bucket: bool) {
        self.from_bucket.store(from_bucket, Ordering::Relaxed);
        self.get_entity().set_synced_data(
            pumpkin_data::tracked_data::axolotl::FROM_BUCKET,
            from_bucket,
        );
    }

    fn bucket_item(&self) -> &'static Item {
        &Item::AXOLOTL_BUCKET
    }

    fn pickup_sound(&self) -> Sound {
        Sound::ItemBucketFillAxolotl
    }

    /// Vanilla `Axolotl.saveToBucketTag`.
    fn save_to_bucket_tag(&self, bucket: &mut ItemStack) {
        self.save_default_data_to_bucket_tag(bucket);
        bucket.set_data_component(AxolotlVariantImpl {
            value: pumpkin_data::entity_variant::AxolotlVariant::by_id(self.get_variant().id())
                .name()
                .into(),
        });
        let mut nbt = bucket
            .get_data_component::<BucketEntityDataImpl>()
            .and_then(|data| data.nbt.clone())
            .unwrap_or_default();
        nbt.put_int("Age", self.get_age());
        nbt.put_bool("AgeLocked", self.is_age_locked());
        let hunting_cooldown = self.hunting_cooldown.load(Ordering::Relaxed);
        if hunting_cooldown > 0 {
            nbt.put_long("HuntingCooldown", i64::from(hunting_cooldown));
        }
        bucket.set_data_component(BucketEntityDataImpl { nbt: Some(nbt) });
    }

    /// Vanilla `Axolotl.loadFromBucketTag`.
    fn load_from_bucket_tag(&self, nbt: &NbtCompound) {
        self.load_default_data_from_bucket_tag(nbt);
        self.set_age(nbt.get_int("Age").unwrap_or(0));
        self.set_age_locked(nbt.get_bool("AgeLocked").unwrap_or(false));
        let hunting_cooldown = nbt
            .get_long("HuntingCooldown")
            .map_or(0, |ttl| ttl.clamp(0, i64::from(i32::MAX)) as i32);
        self.hunting_cooldown
            .store(hunting_cooldown, Ordering::Relaxed);
    }
}
