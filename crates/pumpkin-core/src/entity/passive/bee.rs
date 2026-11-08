use crate::entity::r#type::CreatureSpawnLevel;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicI32, AtomicU8, Ordering},
};

use crossbeam::atomic::AtomicCell;

use pumpkin_data::attributes::Attributes;
use pumpkin_data::block_properties::{DoubleBlockHalf, TallSeagrassLikeProperties};
use pumpkin_data::damage::DamageType;
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::potion::Effect;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::{Block, BlockState};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::Difficulty;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::random::gameplay::{GameRandom, game_random};

use crate::block::entities::beehive::BeehiveBlockEntity;
use crate::enchantment::EnchantmentHelper;
use crate::entity::ai::behavior::hunting::{self, Hunt};
use crate::entity::ai::brain::memory::codec::{block_pos_from_tag, block_pos_to_tag};
use crate::entity::ai::target_match::TargetMatch;
use crate::entity::{
    Entity, EntityBase,
    ageable::{AgeableData, AgeableMob},
    ai::control::{
        Control, LookControlTrait, flying_move_control::FlyingMoveControl,
        look_control::LookControl,
    },
    ai::goal::{
        active_target::TargetCondition, breed::BreedGoal, follow_parent::FollowParentGoal,
        melee_attack::MeleeAttackGoal, reset_universal_anger::ResetUniversalAngerGoal,
        revenge::RevengeGoal, swim::SwimGoal, tempt::TemptGoal,
    },
    ai::pathfinder::{Navigator, node::PathType},
    ai::util::air_random_pos,
    mob::{
        Mob, MobEntity,
        neutral::{NeutralData, NeutralMob},
    },
    passive::animal::Animal,
    player::Player,
};
use crate::world::World;

mod goals;

pub const FLAG_ROLL: u8 = 2;
pub const FLAG_HAS_STUNG: u8 = 4;
pub const FLAG_HAS_NECTAR: u8 = 8;

/// Vanilla `Bee.TICKS_PER_FLAP`, `Mth.ceil(1.4959966F)`.
const TICKS_PER_FLAP: i32 = 2;
const STING_DEATH_COUNTDOWN: i32 = 1200;
const TICKS_BEFORE_GOING_TO_KNOWN_FLOWER: i32 = 600;
const TICKS_WITHOUT_NECTAR_BEFORE_GOING_HOME: i32 = 3600;
const MAX_CROPS_GROWABLE: i32 = 10;
const TOO_FAR_DISTANCE: i32 = 48;
const HIVE_CLOSE_ENOUGH_DISTANCE: i32 = 2;
const RESTRICTED_WANDER_DISTANCE_REDUCTION: i32 = 24;
const DEFAULT_WANDER_DISTANCE_REDUCTION: i32 = 16;
const PATHFIND_TO_HIVE_WHEN_CLOSER_THAN: i32 = 16;
const HIVE_SEARCH_DISTANCE: i32 = 20;
const COOLDOWN_BEFORE_LOCATING_NEW_HIVE: i32 = 200;
const COOLDOWN_BEFORE_LOCATING_NEW_FLOWER: i32 = 200;
const MIN_FIND_FLOWER_RETRY_COOLDOWN: i32 = 20;
const MAX_FIND_FLOWER_RETRY_COOLDOWN: i32 = 60;
/// Vanilla `Bee.customServerAiStep`: ticks in water before the bee drowns.
const MAX_UNDER_WATER_TICKS: i32 = 20;
/// Vanilla `Bee.aiStep`: the bee rolls at a target this close, squared.
const ROLL_DISTANCE_SQ: f64 = 4.0;
/// Vanilla `Bee.aiStep`: how often a bee forgets a hive that is gone.
const HIVE_VALIDATION_INTERVAL: i32 = 20;

fn is_angry(mob: &dyn Mob) -> bool {
    mob.as_neutral().is_some_and(NeutralMob::is_angry)
}

/// Angry and still holding its stinger.
fn can_sting(mob: &dyn Mob) -> bool {
    is_angry(mob)
        && !mob
            .cast_any()
            .downcast_ref::<BeeEntity>()
            .is_some_and(BeeEntity::has_stung)
}

const BEE_HUNTS: &[Hunt] = &[Hunt::new(2, TargetMatch::Type(&EntityType::PLAYER), true)
    .when(TargetCondition::AngryAt)
    .gated_by(can_sting)];

pub struct BeeEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    pub neutral_data: NeutralData,
    pub flags: AtomicU8,
    pub ticks_without_nectar: AtomicI32,
    pub cannot_enter_hive_ticks: AtomicI32,
    pub crops_grown_since_pollination: AtomicI32,
    pub time_since_sting: AtomicI32,
    hive_pos: AtomicCell<Option<BlockPos>>,
    saved_flower_pos: AtomicCell<Option<BlockPos>>,
    remaining_cooldown_before_locating_new_hive: AtomicI32,
    remaining_cooldown_before_locating_new_flower: AtomicI32,
    under_water_ticks: AtomicI32,
    /// Vanilla `BeePollinateGoal.pollinating`, read by the navigation, look control and damage.
    pollinating: AtomicBool,
    /// Vanilla `BeeGoToHiveGoal.blacklistedTargets`, which `BeeLocateHiveGoal` also reads.
    blacklisted_hives: Mutex<Vec<BlockPos>>,
}

impl BeeEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        // Vanilla `Bee.defineSynchedData`.
        entity
            .synched_data
            .define(pumpkin_data::tracked_data::bee::DATA_FLAGS_ID, 0i8);
        entity
            .synched_data
            .define(pumpkin_data::tracked_data::bee::DATA_ANGER_END_TIME, -1i64);
        let mut mob_entity = MobEntity::new(entity);
        *mob_entity
            .move_control
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Box::new(FlyingMoveControl::new(20, true));
        *mob_entity
            .look_control
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Box::new(BeeLookControl::default());
        let navigator = mob_entity
            .navigator
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *navigator = Navigator::flying();
        navigator.set_can_open_doors(false);
        navigator.set_can_float(false);
        navigator.set_required_path_length(48.0);
        navigator.set_pathfinding_malus(PathType::DamageFire, -1.0);
        navigator.set_pathfinding_malus(PathType::Water, -1.0);
        navigator.set_pathfinding_malus(PathType::WaterBorder, 16.0);
        navigator.set_pathfinding_malus(PathType::Cocoa, -1.0);
        navigator.set_pathfinding_malus(PathType::Fence, -1.0);
        let flower_cooldown = random_flower_retry_cooldown(game_random());
        let bee = Self {
            mob_entity,
            ageable_data: AgeableData::default(),
            neutral_data: NeutralData::default(),
            flags: AtomicU8::new(0),
            ticks_without_nectar: AtomicI32::new(0),
            cannot_enter_hive_ticks: AtomicI32::new(0),
            crops_grown_since_pollination: AtomicI32::new(0),
            time_since_sting: AtomicI32::new(0),
            hive_pos: AtomicCell::new(None),
            saved_flower_pos: AtomicCell::new(None),
            remaining_cooldown_before_locating_new_hive: AtomicI32::new(0),
            remaining_cooldown_before_locating_new_flower: AtomicI32::new(flower_cooldown),
            under_water_ticks: AtomicI32::new(0),
            pollinating: AtomicBool::new(false),
            blacklisted_hives: Mutex::new(Vec::new()),
        };
        let mob_arc = Arc::new(bee);

        {
            let goal_selector = &mob_arc.mob_entity.goals_selector;

            goal_selector.add_goal(
                0,
                Box::new(MeleeAttackGoal::new(1.4, true).gated_by(can_sting)),
            );
            goal_selector.add_goal(1, Box::new(goals::BeeEnterHiveGoal));
            goal_selector.add_goal(2, BreedGoal::new(1.0));
            goal_selector.add_goal(
                3,
                Box::new(TemptGoal::with_tag(
                    1.25,
                    &tag::Item::MINECRAFT_BEE_FOOD,
                    false,
                )),
            );
            goal_selector.add_goal(3, Box::new(goals::ValidateHiveGoal::new(&mob_arc)));
            goal_selector.add_goal(3, Box::new(goals::ValidateFlowerGoal::new(&mob_arc)));
            goal_selector.add_goal(4, Box::new(goals::BeePollinateGoal::default()));
            goal_selector.add_goal(5, Box::new(FollowParentGoal::new(1.25)));
            goal_selector.add_goal(5, Box::new(goals::BeeLocateHiveGoal));
            goal_selector.add_goal(5, Box::new(goals::BeeGoToHiveGoal::default()));
            goal_selector.add_goal(6, Box::new(goals::BeeGoToKnownFlowerGoal::default()));
            goal_selector.add_goal(7, Box::new(goals::BeeGrowCropGoal));
            goal_selector.add_goal(8, Box::new(goals::BeeWanderGoal));
            goal_selector.add_goal(9, Box::new(SwimGoal::default()));
        };

        {
            let target_selector = &mob_arc.mob_entity.target_selector;

            // Vanilla BeeHurtByOtherGoal.
            target_selector.add_goal(
                1,
                Box::new(
                    RevengeGoal::new(true)
                        .alerting_others()
                        .alerting_in_sight()
                        .continuing_while(is_angry),
                ),
            );
            hunting::apply(target_selector, &mob_arc.mob_entity, BEE_HUNTS);
            target_selector.add_goal(3, ResetUniversalAngerGoal::new(true));
        };

        mob_arc
    }

    #[must_use]
    pub fn has_flag(&self, flag: u8) -> bool {
        (self.flags.load(Ordering::Relaxed) & flag) != 0
    }

    pub fn set_flag(&self, flag: u8, val: bool) {
        let current = self.flags.load(Ordering::Relaxed);
        let new_flags = if val { current | flag } else { current & !flag };
        self.flags.store(new_flags, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::bee::DATA_FLAGS_ID,
            new_flags as i8,
        );
    }

    #[must_use]
    pub fn has_nectar(&self) -> bool {
        self.has_flag(FLAG_HAS_NECTAR)
    }

    pub fn set_has_nectar(&self, val: bool) {
        if val {
            self.reset_ticks_without_nectar_since_exiting_hive();
        }
        self.set_flag(FLAG_HAS_NECTAR, val);
    }

    #[must_use]
    pub fn has_stung(&self) -> bool {
        self.has_flag(FLAG_HAS_STUNG)
    }

    pub fn set_has_stung(&self, val: bool) {
        self.set_flag(FLAG_HAS_STUNG, val);
    }

    #[must_use]
    pub fn is_rolling(&self) -> bool {
        self.has_flag(FLAG_ROLL)
    }

    pub fn set_rolling(&self, val: bool) {
        self.set_flag(FLAG_ROLL, val);
    }

    #[must_use]
    pub fn hive_pos(&self) -> Option<BlockPos> {
        self.hive_pos.load()
    }

    pub fn set_hive_pos(&self, pos: Option<BlockPos>) {
        self.hive_pos.store(pos);
    }

    #[must_use]
    pub fn saved_flower_pos(&self) -> Option<BlockPos> {
        self.saved_flower_pos.load()
    }

    pub fn set_saved_flower_pos(&self, pos: Option<BlockPos>) {
        self.saved_flower_pos.store(pos);
    }

    fn reset_ticks_without_nectar_since_exiting_hive(&self) {
        self.ticks_without_nectar.store(0, Ordering::Relaxed);
    }

    /// Vanilla `dropOffNectar`.
    pub fn drop_off_nectar(&self) {
        self.set_has_nectar(false);
        self.crops_grown_since_pollination
            .store(0, Ordering::Relaxed);
    }

    fn is_pollinating(&self) -> bool {
        self.pollinating.load(Ordering::Relaxed)
    }

    fn is_tired_of_looking_for_nectar(&self) -> bool {
        self.ticks_without_nectar.load(Ordering::Relaxed) > TICKS_WITHOUT_NECTAR_BEFORE_GOING_HOME
    }

    /// Vanilla `dropHive`.
    fn drop_hive(&self) {
        self.hive_pos.store(None);
        self.remaining_cooldown_before_locating_new_hive
            .store(COOLDOWN_BEFORE_LOCATING_NEW_HIVE, Ordering::Relaxed);
    }

    /// Vanilla `dropFlower`.
    fn drop_flower(&self) {
        self.saved_flower_pos.store(None);
        self.remaining_cooldown_before_locating_new_flower.store(
            random_flower_retry_cooldown(self.get_random()),
            Ordering::Relaxed,
        );
    }

    /// Vanilla `closerThan`: the block distance from the bee's block to `pos` is under `distance`.
    fn closer_than(&self, pos: BlockPos, distance: i32) -> bool {
        pos.squared_distance(&self.get_entity().block_pos.load()) < distance * distance
    }

    /// Vanilla `isTooFarAway`.
    fn is_too_far_away(&self, pos: BlockPos) -> bool {
        !self.closer_than(pos, TOO_FAR_DISTANCE)
    }

    /// Vanilla `getBeehiveBlockEntity`: the remembered hive, if it is near and still a hive.
    fn with_beehive<R>(&self, f: impl FnOnce(&BeehiveBlockEntity) -> R) -> Option<R> {
        let hive_pos = self.hive_pos()?;
        if self.is_too_far_away(hive_pos) {
            return None;
        }
        let block_entity = self.get_entity().world.load().get_block_entity(&hive_pos)?;
        block_entity
            .as_any()
            .downcast_ref::<BeehiveBlockEntity>()
            .map(f)
    }

    /// Vanilla `isHiveValid`.
    fn is_hive_valid(&self) -> bool {
        self.with_beehive(|_| ()).is_some()
    }

    /// Vanilla `isHiveNearFire`.
    fn is_hive_near_fire(&self) -> bool {
        let world = self.get_entity().world.load();
        self.with_beehive(|hive| hive.is_fire_nearby(&world))
            .unwrap_or(false)
    }

    /// Vanilla `doesHiveHaveSpace`.
    fn does_hive_have_space(world: &World, pos: &BlockPos) -> bool {
        world.get_block_entity(pos).is_some_and(|block_entity| {
            block_entity
                .as_any()
                .downcast_ref::<BeehiveBlockEntity>()
                .is_some_and(|hive| !hive.is_full())
        })
    }

    /// Vanilla `wantsToEnterHive`.
    fn wants_to_enter_hive(&self) -> bool {
        if self.cannot_enter_hive_ticks.load(Ordering::Relaxed) > 0
            || self.is_pollinating()
            || self.has_stung()
            || self.mob_entity.get_target().is_some()
        {
            return false;
        }
        let entity = self.get_entity();
        let wants_to_enter_hive = self.has_nectar()
            || self.is_tired_of_looking_for_nectar()
            || entity
                .world
                .load()
                .bees_stay_in_hive(&entity.block_pos.load());
        wants_to_enter_hive && !self.is_hive_near_fire()
    }

    /// Vanilla `pathfindRandomlyTowards`: flies a random step towards `target`.
    fn pathfind_randomly_towards(&self, target: BlockPos) {
        let target_vec = Vector3::new(
            f64::from(target.0.x) + 0.5,
            f64::from(target.0.y),
            f64::from(target.0.z) + 0.5,
        );
        let bee_pos = self.get_entity().block_pos.load();
        let y_delta = target_vec.y as i32 - bee_pos.0.y;
        let y_adjust = if y_delta > 2 {
            4
        } else if y_delta < -2 {
            -4
        } else {
            0
        };
        let mut xz_dist = 6;
        let mut y_dist = 8;
        let dist = bee_pos.manhattan_distance(target);
        if dist < 15 {
            xz_dist = dist / 2;
            y_dist = dist / 2;
        }
        if let Some(next) = air_random_pos::get_pos_towards(
            self,
            xz_dist,
            y_dist,
            y_adjust,
            target_vec,
            std::f64::consts::PI / 10.0,
        ) {
            let living = &self.mob_entity.living_entity;
            let mut navigator = self
                .mob_entity
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            navigator.set_max_visited_nodes_multiplier(0.5);
            navigator.move_to_coords(next.x, next.y, next.z, 1.0, living);
        }
    }

    /// Vanilla `attractsBees`.
    #[must_use]
    pub fn attracts_bees(state: &BlockState) -> bool {
        let block = Block::from_state_id(state.id);
        if !block.has_tag(&tag::Block::MINECRAFT_BEE_ATTRACTIVE) || state.is_waterlogged() {
            return false;
        }
        block != &Block::SUNFLOWER
            || TallSeagrassLikeProperties::from_state_id(state.id).half == DoubleBlockHalf::Upper
    }
}

/// Vanilla `Mth.nextInt(random, MIN_FIND_FLOWER_RETRY_COOLDOWN, MAX_FIND_FLOWER_RETRY_COOLDOWN)`.
fn random_flower_retry_cooldown(random: GameRandom) -> i32 {
    random.random_range(MIN_FIND_FLOWER_RETRY_COOLDOWN..=MAX_FIND_FLOWER_RETRY_COOLDOWN)
}

impl AgeableMob for BeeEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }
}

impl Animal for BeeEntity {
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack.item.has_tag(&tag::Item::MINECRAFT_FLOWERS)
            || item_stack.item.has_tag(&tag::Item::MINECRAFT_BEE_FOOD)
    }
}

impl NeutralMob for BeeEntity {
    fn updates_anger_in_ai_step(&self) -> bool {
        true
    }

    fn get_neutral_data(&self) -> &NeutralData {
        &self.neutral_data
    }

    fn anger_end_time_data(&self) -> Option<pumpkin_data::tracked_data::TrackedData> {
        Some(pumpkin_data::tracked_data::bee::DATA_ANGER_END_TIME)
    }

    /// Bees calm down once the target is gone, unlike the other neutral mobs.
    fn stays_angry_with_target(&self) -> bool {
        false
    }
}

#[derive(Default)]
struct BeeLookControl(LookControl);

impl Control for BeeLookControl {}

impl LookControlTrait for BeeLookControl {
    fn tick(&mut self, mob: &dyn Mob) {
        if !is_angry(mob) {
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

impl Mob for BeeEntity {
    /// Vanilla `Bee.getWalkTargetValue`.
    fn get_walk_target_value(&self, level: &dyn CreatureSpawnLevel, pos: &BlockPos) -> f32 {
        if level.block_state(pos).is_air() {
            10.0
        } else {
            0.0
        }
    }

    fn mob_is_flapping(&self) -> bool {
        let entity = self.get_entity();
        !entity.on_ground.load(Ordering::Relaxed)
            && entity.tick_count.load(Ordering::Relaxed) % TICKS_PER_FLAP == 0
    }

    /// Vanilla `Bee` steps silently.
    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {}

    fn omnidirectional_air_mover(&self) -> bool {
        true
    }

    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn as_animal(&self) -> Option<&dyn Animal> {
        Some(self)
    }

    fn as_neutral(&self) -> Option<&dyn NeutralMob> {
        Some(self)
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        if let Some(hive_pos) = self.hive_pos() {
            nbt.put("hive_pos", block_pos_to_tag(hive_pos));
        }
        if let Some(flower_pos) = self.saved_flower_pos() {
            nbt.put("flower_pos", block_pos_to_tag(flower_pos));
        }
        nbt.put_bool("HasNectar", self.has_nectar());
        nbt.put_bool("HasStung", self.has_stung());
        nbt.put_int(
            "TicksSincePollination",
            self.ticks_without_nectar.load(Ordering::Relaxed),
        );
        nbt.put_int(
            "CannotEnterHiveTicks",
            self.cannot_enter_hive_ticks.load(Ordering::Relaxed),
        );
        nbt.put_int(
            "CropsGrownSincePollination",
            self.crops_grown_since_pollination.load(Ordering::Relaxed),
        );
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        if let Some(nectar) = nbt.get_bool("HasNectar") {
            self.set_has_nectar(nectar);
        }
        if let Some(stung) = nbt.get_bool("HasStung") {
            self.set_has_stung(stung);
        }
        if let Some(ticks) = nbt.get_int("TicksSincePollination") {
            self.ticks_without_nectar.store(ticks, Ordering::Relaxed);
        }
        if let Some(cannot) = nbt.get_int("CannotEnterHiveTicks") {
            self.cannot_enter_hive_ticks
                .store(cannot, Ordering::Relaxed);
        }
        if let Some(crops) = nbt.get_int("CropsGrownSincePollination") {
            self.crops_grown_since_pollination
                .store(crops, Ordering::Relaxed);
        }
        self.hive_pos
            .store(nbt.get("hive_pos").and_then(block_pos_from_tag));
        self.saved_flower_pos
            .store(nbt.get("flower_pos").and_then(block_pos_from_tag));
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    /// Vanilla `Bee.doHurtTarget`: a sting that poisons and leaves the bee without its stinger.
    fn do_hurt_target(&self, target: &dyn EntityBase) -> bool {
        let damage = self
            .mob_entity
            .living_entity
            .get_attribute_value(&Attributes::ATTACK_DAMAGE) as i32 as f32;
        let was_hurt = target.damage_with_context(
            target,
            damage,
            DamageType::STING,
            None,
            Some(self),
            Some(self),
        );
        if !was_hurt {
            return false;
        }
        EnchantmentHelper::do_post_attack_effects(
            target,
            DamageType::STING,
            Some(self),
            Some(self),
        );
        if let Some(living) = target.get_living_entity() {
            let world = self.get_entity().world.load();
            let poison_seconds = match world.level_info.load().difficulty {
                Difficulty::Normal => 10,
                Difficulty::Hard => 18,
                Difficulty::Easy | Difficulty::Peaceful => 0,
            };
            if poison_seconds > 0 {
                living.add_effect_with_source(
                    Effect {
                        effect_type: &StatusEffect::POISON,
                        duration: poison_seconds * 20,
                        amplifier: 0,
                        ambient: false,
                        show_particles: true,
                        show_icon: true,
                        blend: false,
                    },
                    Some(self),
                );
            }
        }
        self.set_has_stung(true);
        self.stop_being_angry();
        self.play_entity_sound(Sound::EntityBeeSting, 1.0, 1.0);
        true
    }

    /// Vanilla `Bee.customServerAiStep`.
    fn custom_server_ai_step(&self, caller: &dyn EntityBase) {
        let has_stung = self.has_stung();
        if self.get_entity().is_in_water() {
            self.under_water_ticks.fetch_add(1, Ordering::Relaxed);
        } else {
            self.under_water_ticks.store(0, Ordering::Relaxed);
        }
        if self.under_water_ticks.load(Ordering::Relaxed) > MAX_UNDER_WATER_TICKS {
            caller.damage(caller, 1.0, DamageType::DROWN);
        }
        if has_stung {
            let time = self.time_since_sting.fetch_add(1, Ordering::Relaxed) + 1;
            if time % 5 == 0
                && self
                    .get_random()
                    .random_range(0..(STING_DEATH_COUNTDOWN - time).clamp(1, STING_DEATH_COUNTDOWN))
                    == 0
            {
                let health = self.mob_entity.living_entity.health.load();
                caller.damage(caller, health, DamageType::GENERIC);
            }
        }
        if !self.has_nectar() {
            self.ticks_without_nectar.fetch_add(1, Ordering::Relaxed);
        }
        self.update_persistent_anger();
    }

    /// The server half of vanilla `Bee.aiStep`.
    fn mob_ai_step(&self) {
        for counter in [
            &self.cannot_enter_hive_ticks,
            &self.remaining_cooldown_before_locating_new_hive,
            &self.remaining_cooldown_before_locating_new_flower,
        ] {
            if counter.load(Ordering::Relaxed) > 0 {
                counter.fetch_sub(1, Ordering::Relaxed);
            }
        }
        let entity = self.get_entity();
        let should_roll = is_angry(self)
            && !self.has_stung()
            && self.mob_entity.get_target().is_some_and(|target| {
                target
                    .get_entity()
                    .pos
                    .load()
                    .squared_distance_to_vec(&entity.pos.load())
                    < ROLL_DISTANCE_SQ
            });
        if should_roll != self.is_rolling() {
            self.set_rolling(should_roll);
        }
        if entity.tick_count.load(Ordering::Relaxed) % HIVE_VALIDATION_INTERVAL == 0
            && !self.is_hive_valid()
        {
            self.hive_pos.store(None);
        }
    }

    /// Vanilla `Bee.hurtServer`: any damage it isn't immune to interrupts pollination.
    fn pre_damage_with_context(
        &self,
        damage_type: DamageType,
        _source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> bool {
        if self.get_entity().is_invulnerable_to(&damage_type, cause) {
            return false;
        }
        self.pollinating.store(false, Ordering::Relaxed);
        true
    }

    /// Vanilla `Bee.createNavigation`: the navigation pauses while the bee pollinates.
    fn ticks_navigation(&self) -> bool {
        !self.is_pollinating()
    }

    /// Vanilla bee navigation's `isStableDestination`: anything above a non-air block.
    fn is_stable_destination(&self, world: &World, pos: &BlockPos) -> bool {
        !world.get_block_state(&pos.down()).is_air()
    }

    /// Vanilla `BeeLookControl.resetXRotOnTick`.
    fn look_resets_pitch(&self) -> bool {
        !self.is_pollinating()
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        let is_baby = entity.age.load(Ordering::Relaxed) < 0;
        if is_baby {
            entity.set_synced_data(pumpkin_data::tracked_data::bee::DATA_BABY_ID, true);
        }
        entity.set_synced_data(
            pumpkin_data::tracked_data::bee::DATA_FLAGS_ID,
            self.flags.load(Ordering::Relaxed) as i8,
        );
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        self.animal_interact(player, item_stack)
    }
}
