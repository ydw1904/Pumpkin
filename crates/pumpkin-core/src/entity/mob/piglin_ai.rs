use pumpkin_util::random::gameplay::game_random;
use pumpkin_util::random::{RandomImpl, legacy_rand::LegacyRand};
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use crossbeam::atomic::AtomicCell;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use uuid::Uuid;

use pumpkin_data::attributes::Attributes;
use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_util::math::vector3::Vector3;

use crate::entity::Entity;
use crate::entity::EntityBase;
use crate::entity::ai::goal::{Controls, Goal};
use crate::entity::ai::random_pos::land_random_pos;
use crate::entity::ai::target_predicate::TargetPredicate;
use crate::entity::item::ItemEntity;
use crate::entity::living::LivingEntity;
use crate::entity::mob::Mob;
use crate::entity::mob::hoglin::HoglinEntity;
use crate::entity::mob::neutral::find_by_uuid;
use crate::entity::mob::piglin::{PiglinActivity, PiglinEntity};
use crate::entity::mob::piglin_brute::PiglinBruteEntity;
use crate::entity::player::Player;
use crate::plugin::api::events::entity::entity_target::TargetReason;
use crate::world::World;

pub struct PiglinAi;

impl PiglinAi {
    pub const REPELLENT_DETECTION_RANGE_HORIZONTAL: i32 = 8;
    pub const REPELLENT_DETECTION_RANGE_VERTICAL: i32 = 4;
    pub const BARTERING_ITEM: &'static Item = &Item::GOLD_INGOT;
    pub const PLAYER_ANGER_RANGE: f64 = 16.0;
    pub const ANGER_DURATION: i32 = 600;
    pub const ADMIRE_DURATION: i32 = 119;
    pub const ADMIRING_DISABLED_DURATION: i32 = 400;
    pub const EAT_COOLDOWN: i32 = 200;
    pub const BABY_FLEE_DURATION: i32 = 100;
    pub const CELEBRATION_TIME: i32 = 300;
    pub const MIN_TIME_BETWEEN_HUNTS: i32 = 600;
    pub const MAX_TIME_BETWEEN_HUNTS: i32 = 2400;
    pub const DESIRED_DISTANCE_FROM_ZOMBIFIED: f64 = 6.0;
    pub const DESIRED_DISTANCE_FROM_ENTITY_WHEN_AVOIDING: i32 = 12;
    pub const MAX_WALK_DISTANCE_TO_START_RIDING: f64 = 8.0;
    pub const MAX_PASSENGERS_ON_ONE_HOGLIN: usize = 3;
    /// `RIDE_START_INTERVAL`, 10 to 40 seconds.
    pub const RIDE_START_INTERVAL: (i32, i32) = (200, 800);
    /// `RIDE_DURATION`, 10 to 30 seconds.
    pub const RIDE_DURATION: (i32, i32) = (200, 600);
    /// `RETREAT_DURATION`, 5 to 20 seconds.
    pub const RETREAT_DURATION: (i32, i32) = (100, 400);
    /// `AVOID_ZOMBIFIED_DURATION`, 5 to 7 seconds.
    pub const AVOID_ZOMBIFIED_DURATION: (i32, i32) = (100, 140);
    /// `BABY_AVOID_NEMESIS_DURATION`, 5 to 7 seconds.
    pub const BABY_AVOID_NEMESIS_DURATION: (i32, i32) = (100, 140);
    pub const SPEED_MULTIPLIER_WHEN_AVOIDING: f64 = 1.0;
    pub const SPEED_MULTIPLIER_WHEN_MOUNTING: f64 = 0.8;
    pub const SPEED_MULTIPLIER_WHEN_GOING_TO_CELEBRATE_LOCATION: f64 = 1.0;
    pub const SPEED_MULTIPLIER_WHEN_DANCING: f64 = 0.6;
    pub const PROBABILITY_OF_CELEBRATION_DANCE: f32 = 0.1;
    pub const ITEM_SCAN_RANGE: f64 = 32.0;
    pub const ITEM_SCAN_RANGE_Y: f64 = 16.0;
    pub const SENSOR_SCAN_INTERVAL: i64 = 20;
    pub const MAX_DISTANCE_TO_WALK_TO_ITEM: f64 = 9.0;
    pub const MAX_TIME_TRYING_TO_REACH_ITEM: i32 = 200;
    pub const DISABLE_WALK_TO_ADMIRE_DURATION: i32 = 200;
    pub const THROW_SPEED: f64 = 0.3;
    pub const THROW_HAND_Y_DISTANCE_FROM_EYE: f64 = 0.3;
    pub const RANDOM_THROW_POS_RANGE_HORIZONTAL: i32 = 4;
    pub const RANDOM_THROW_POS_RANGE_VERTICAL: i32 = 2;

    #[must_use]
    pub const fn is_barter_currency(item_stack: &ItemStack) -> bool {
        item_stack.item.id == Self::BARTERING_ITEM.id
    }

    #[must_use]
    pub fn is_loved_item(item_stack: &ItemStack) -> bool {
        item_stack.item.has_tag(&tag::Item::MINECRAFT_PIGLIN_LOVED)
    }

    #[must_use]
    pub fn is_food(item_stack: &ItemStack) -> bool {
        item_stack.item.has_tag(&tag::Item::MINECRAFT_PIGLIN_FOOD)
    }

    #[must_use]
    pub const fn is_zombified(entity_type: &EntityType) -> bool {
        entity_type.id == EntityType::ZOMBIFIED_PIGLIN.id || entity_type.id == EntityType::ZOGLIN.id
    }

    /// Vanilla `PiglinAi.wantsToDance`: the roll is seeded with the game time, so every piglin
    /// whose target dies in the same tick makes the same choice.
    #[must_use]
    pub fn wants_to_dance(killed_target_type: &EntityType, game_time: i64) -> bool {
        if killed_target_type.id != EntityType::HOGLIN.id {
            return false;
        }
        LegacyRand::from_seed(game_time as u64).next_f32() < Self::PROBABILITY_OF_CELEBRATION_DANCE
    }

    #[must_use]
    pub fn is_wearing_safe_armor(entity: &LivingEntity) -> bool {
        let Ok(guard) = entity.entity_equipment.try_lock() else {
            return false;
        };

        for slot in [
            EquipmentSlot::HEAD,
            EquipmentSlot::CHEST,
            EquipmentSlot::LEGS,
            EquipmentSlot::FEET,
        ] {
            if let Some(stack) = guard.equipment.get(&slot)
                && !stack.is_empty()
                && (stack.item.has_tag(&tag::Item::MINECRAFT_PIGLIN_SAFE_ARMOR)
                    || stack.item.has_tag(&tag::Item::MINECRAFT_PIGLIN_LOVED))
            {
                return true;
            }
        }
        false
    }

    #[must_use]
    pub fn is_holding_loved_item(entity: &LivingEntity) -> bool {
        let Ok(guard) = entity.entity_equipment.try_lock() else {
            return false;
        };
        for slot in [EquipmentSlot::MAIN_HAND, EquipmentSlot::OFF_HAND] {
            if let Some(stack) = guard.equipment.get(&slot)
                && !stack.is_empty()
                && Self::is_loved_item(stack)
            {
                return true;
            }
        }
        false
    }

    #[must_use]
    pub fn is_player_holding_loved_item(player: &Player) -> bool {
        let inventory = player.inventory();
        Self::is_loved_item(&inventory.held_item())
            || Self::is_loved_item(&inventory.off_hand_item())
    }

    #[must_use]
    pub fn is_admiring_disabled(piglin: &PiglinEntity) -> bool {
        piglin.is_admiring_disabled()
    }

    #[must_use]
    pub fn can_admire(piglin: &PiglinEntity, item_stack: &ItemStack) -> bool {
        !Self::is_admiring_disabled(piglin)
            && !piglin.is_admiring()
            && piglin.is_adult()
            && Self::is_barter_currency(item_stack)
    }

    #[must_use]
    pub fn is_not_holding_loved_item_in_off_hand(piglin: &PiglinEntity) -> bool {
        let off_hand = piglin.off_hand_item();
        off_hand.is_empty() || !Self::is_loved_item(&off_hand)
    }

    #[must_use]
    pub fn wants_to_pickup(piglin: &PiglinEntity, item_stack: &ItemStack) -> bool {
        if piglin.is_baby()
            && item_stack
                .item
                .has_tag(&tag::Item::MINECRAFT_IGNORED_BY_PIGLIN_BABIES)
        {
            return false;
        }
        if item_stack
            .item
            .has_tag(&tag::Item::MINECRAFT_PIGLIN_REPELLENTS)
        {
            return false;
        }
        if piglin.is_admiring_disabled() && piglin.has_attack_target() {
            return false;
        }
        if Self::is_barter_currency(item_stack) {
            return Self::is_not_holding_loved_item_in_off_hand(piglin);
        }

        let has_space = piglin.can_add_to_inventory(item_stack);
        if item_stack.item.id == Item::GOLD_NUGGET.id {
            return has_space;
        }
        if Self::is_food(item_stack) {
            return !piglin.has_eaten_recently() && has_space;
        }
        if !Self::is_loved_item(item_stack) {
            return piglin.can_replace_current_item_for(item_stack);
        }
        Self::is_not_holding_loved_item_in_off_hand(piglin) && has_space
    }

    #[must_use]
    pub fn get_barter_response_items() -> Vec<ItemStack> {
        let roll = game_random().random_range(0..459);
        match roll {
            0..5 => vec![ItemStack::new(1, &Item::ENCHANTED_BOOK)],
            5..13 => vec![ItemStack::new(1, &Item::IRON_BOOTS)],
            13..21 => vec![ItemStack::new(1, &Item::SPLASH_POTION)],
            21..39 => vec![ItemStack::new(1, &Item::POTION)],
            39..49 => vec![ItemStack::new(
                game_random().random_range(10..=36),
                &Item::IRON_NUGGET,
            )],
            49..59 => vec![ItemStack::new(
                game_random().random_range(2..=4),
                &Item::ENDER_PEARL,
            )],
            59..79 => vec![ItemStack::new(
                game_random().random_range(3..=9),
                &Item::STRING,
            )],
            79..99 => vec![ItemStack::new(
                game_random().random_range(5..=12),
                &Item::QUARTZ,
            )],
            99..139 => vec![ItemStack::new(1, &Item::OBSIDIAN)],
            139..179 => vec![ItemStack::new(
                game_random().random_range(1..=3),
                &Item::CRYING_OBSIDIAN,
            )],
            179..219 => vec![ItemStack::new(1, &Item::FIRE_CHARGE)],
            219..259 => vec![ItemStack::new(
                game_random().random_range(2..=4),
                &Item::LEATHER,
            )],
            259..299 => vec![ItemStack::new(
                game_random().random_range(2..=8),
                &Item::SOUL_SAND,
            )],
            299..339 => vec![ItemStack::new(
                game_random().random_range(2..=8),
                &Item::NETHER_BRICK,
            )],
            339..379 => vec![ItemStack::new(
                game_random().random_range(6..=12),
                &Item::SPECTRAL_ARROW,
            )],
            379..419 => vec![ItemStack::new(
                game_random().random_range(8..=16),
                &Item::GRAVEL,
            )],
            _ => vec![ItemStack::new(
                game_random().random_range(8..=16),
                &Item::BLACKSTONE,
            )],
        }
    }

    #[must_use]
    pub fn get_sound_for_activity(piglin: &PiglinEntity, activity: PiglinActivity) -> Sound {
        if activity == PiglinActivity::Fight {
            return Sound::EntityPiglinAngry;
        }
        let world = piglin.mob_entity.living_entity.entity.world.load();
        if piglin.is_converting(&world)
            || (activity == PiglinActivity::Avoid && piglin.is_near_avoid_target())
        {
            return Sound::EntityPiglinRetreat;
        }
        match activity {
            PiglinActivity::AdmireItem => Sound::EntityPiglinAdmiringItem,
            PiglinActivity::Celebrate => Sound::EntityPiglinCelebrate,
            _ if Self::sees_player_holding_loved_item(piglin) => Sound::EntityPiglinJealous,
            _ if piglin.is_near_repellent() => Sound::EntityPiglinRetreat,
            _ => Sound::EntityPiglinAmbient,
        }
    }

    fn sees_player_holding_loved_item(piglin: &PiglinEntity) -> bool {
        piglin
            .nearest_visible_player()
            .is_some_and(|player| Self::is_player_holding_loved_item(&player))
    }

    /// Vanilla `Sensor.isEntityTargetable`: non-combat test within the follow range,
    /// unscaled by sneaking and invisibility for the current attack target.
    #[must_use]
    pub fn is_entity_targetable(piglin: &PiglinEntity, target: &Player) -> bool {
        let living = &piglin.mob_entity.living_entity;
        let target_id = target.get_entity().entity_id;
        let is_attack_target = piglin
            .mob_entity
            .target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .is_some_and(|current| current.get_entity().entity_id == target_id);
        let conditions = TargetPredicate::create_non_attackable()
            .set_base_max_distance(living.get_attribute_value(&Attributes::FOLLOW_RANGE));
        let conditions = if is_attack_target {
            conditions.ignore_distance_scaling_factor()
        } else {
            conditions
        };
        let world = living.entity.world.load();
        conditions.test(&world, Some(piglin), target)
    }

    pub fn throw_items(piglin: &PiglinEntity, items: Vec<ItemStack>) {
        match piglin.nearest_visible_player() {
            Some(player) => {
                Self::throw_items_toward_pos(piglin, items, player.get_entity().pos.load());
            }
            None => Self::throw_items_toward_random_pos(piglin, items),
        }
    }

    pub fn throw_items_toward_random_pos(piglin: &PiglinEntity, items: Vec<ItemStack>) {
        Self::throw_items_toward_pos(piglin, items, Self::random_nearby_pos(piglin));
    }

    fn throw_items_toward_pos(piglin: &PiglinEntity, items: Vec<ItemStack>, target: Vector3<f64>) {
        if items.is_empty() {
            return;
        }
        let living = &piglin.mob_entity.living_entity;
        living.swing_off_hand();

        let entity = &living.entity;
        let world = entity.world.load();
        let pos = entity.pos.load();
        let hand_pos = Vector3::new(
            pos.x,
            entity.get_eye_y() - Self::THROW_HAND_Y_DISTANCE_FROM_EYE,
            pos.z,
        );
        let direction =
            Vector3::new(target.x - pos.x, target.y + 1.0 - pos.y, target.z - pos.z).normalize();
        let velocity = Vector3::new(
            direction.x * Self::THROW_SPEED,
            direction.y * Self::THROW_SPEED,
            direction.z * Self::THROW_SPEED,
        );

        for item in items {
            if item.is_empty() {
                continue;
            }
            let item_entity = ItemEntity::new_with_velocity(
                Entity::new(world.clone(), hand_pos, &EntityType::ITEM),
                item,
                velocity,
                ItemEntity::DEFAULT_PICKUP_DELAY,
            );
            world.spawn_entity(Arc::new(item_entity));
        }
    }

    fn random_nearby_pos(piglin: &PiglinEntity) -> Vector3<f64> {
        land_random_pos(
            piglin,
            Self::RANDOM_THROW_POS_RANGE_HORIZONTAL,
            Self::RANDOM_THROW_POS_RANGE_VERTICAL,
        )
        .unwrap_or_else(|| piglin.mob_entity.living_entity.entity.pos.load())
    }
}

/// The `ANGRY_AT` and `UNIVERSAL_ANGER` brain memories of piglins and piglin brutes.
#[derive(Default)]
pub struct PiglinAnger {
    angry_at: AtomicCell<Option<Uuid>>,
    angry_at_ttl: AtomicI32,
    universal_anger_ttl: AtomicI32,
}

impl PiglinAnger {
    const ANGRY_AT_MEMORY: &'static str = "minecraft:angry_at";
    const UNIVERSAL_ANGER_MEMORY: &'static str = "minecraft:universal_anger";

    #[must_use]
    pub fn angry_at(&self) -> Option<Uuid> {
        self.angry_at.load()
    }

    #[must_use]
    pub fn has_universal_anger(&self) -> bool {
        self.universal_anger_ttl.load(Ordering::Relaxed) > 0
    }

    pub fn erase_angry_at(&self) {
        self.angry_at.store(None);
        self.angry_at_ttl.store(0, Ordering::Relaxed);
    }

    /// Vanilla `Brain.forgetOutdatedMemories` for the two memories.
    pub fn tick(&self) {
        if self.angry_at_ttl.load(Ordering::Relaxed) > 0
            && self.angry_at_ttl.fetch_sub(1, Ordering::Relaxed) <= 1
        {
            self.angry_at.store(None);
        }
        if self.universal_anger_ttl.load(Ordering::Relaxed) > 0 {
            self.universal_anger_ttl.fetch_sub(1, Ordering::Relaxed);
        }
    }

    /// Vanilla `StopBeingAngryIfTargetDead`.
    pub fn stop_being_angry_if_target_dead(&self, world: &World) {
        let Some(uuid) = self.angry_at() else {
            return;
        };
        let Some(target) = find_by_uuid(world, uuid) else {
            return;
        };
        let Some(living) = target.get_living_entity() else {
            return;
        };
        if living.is_dead_or_dying()
            && (target.get_player().is_none()
                || world.level_info.load().game_rules.forgive_dead_players)
        {
            self.erase_angry_at();
        }
    }

    pub fn write_memories(&self, memories: &mut NbtCompound) {
        if let Some(uuid) = self.angry_at() {
            let mut memory = NbtCompound::new();
            memory.put_uuid("value", uuid);
            memory.put_long("ttl", i64::from(self.angry_at_ttl.load(Ordering::Relaxed)));
            memories.put(Self::ANGRY_AT_MEMORY, NbtTag::Compound(memory));
        }
        let universal = self.universal_anger_ttl.load(Ordering::Relaxed);
        if universal > 0 {
            let mut memory = NbtCompound::new();
            memory.put_bool("value", true);
            memory.put_long("ttl", i64::from(universal));
            memories.put(Self::UNIVERSAL_ANGER_MEMORY, NbtTag::Compound(memory));
        }
    }

    pub fn read_memories(&self, memories: &NbtCompound) {
        let ttl = |memory: &NbtCompound| {
            memory
                .get_long("ttl")
                .map_or(i32::MAX, |ttl| ttl.clamp(0, i64::from(i32::MAX)) as i32)
        };
        if let Some(memory) = memories.get_compound(Self::ANGRY_AT_MEMORY)
            && let Some(uuid) = memory.get_uuid("value")
        {
            self.angry_at.store(Some(uuid));
            self.angry_at_ttl.store(ttl(memory), Ordering::Relaxed);
        }
        if let Some(memory) = memories.get_compound(Self::UNIVERSAL_ANGER_MEMORY) {
            self.universal_anger_ttl
                .store(ttl(memory), Ordering::Relaxed);
        }
    }

    fn set_angry_at(&self, uuid: Uuid) {
        self.angry_at.store(Some(uuid));
        self.angry_at_ttl
            .store(PiglinAi::ANGER_DURATION, Ordering::Relaxed);
    }

    fn set_universal_anger(&self) {
        self.universal_anger_ttl
            .store(PiglinAi::ANGER_DURATION, Ordering::Relaxed);
    }
}

/// Vanilla `AbstractPiglin`: what `PiglinAi`'s anger code needs from piglins and brutes.
pub trait AbstractPiglin: Mob {
    fn piglin_anger(&self) -> &PiglinAnger;

    fn is_adult_piglin(&self) -> bool;

    /// Vanilla `AbstractPiglin.canHunt`.
    fn can_hunt(&self) -> bool;

    /// The `HUNTED_RECENTLY` memory, which brutes don't have.
    fn hunted_recently(&self) -> bool {
        false
    }

    /// Vanilla `dontKillAnyMoreHoglinsForAWhile`; brutes have no `HUNTED_RECENTLY` memory.
    fn dont_kill_any_more_hoglins_for_a_while(&self) {}

    /// Vanilla `PiglinAi.isIdle`.
    fn is_idle(&self) -> bool;

    /// Whether the activity runs `StartAttacking` and `findNearestValidAttackTarget` may find
    /// a target.
    fn can_start_attacking(&self) -> bool {
        true
    }
}

#[must_use]
pub fn as_abstract_piglin(entity: &dyn EntityBase) -> Option<&dyn AbstractPiglin> {
    let any = entity.cast_any();
    if let Some(piglin) = any.downcast_ref::<PiglinEntity>() {
        return Some(piglin);
    }
    any.downcast_ref::<PiglinBruteEntity>()
        .map(|brute| brute as &dyn AbstractPiglin)
}

fn sort_by_distance<T: EntityBase + ?Sized>(entities: &mut [Arc<T>], pos: &Vector3<f64>) {
    entities.sort_by(|a, b| {
        a.get_entity()
            .pos
            .load()
            .squared_distance_to_vec(pos)
            .total_cmp(&b.get_entity().pos.load().squared_distance_to_vec(pos))
    });
}

impl PiglinAi {
    fn follow_range(body: &dyn AbstractPiglin) -> f64 {
        body.get_mob_entity()
            .living_entity
            .get_attribute_value(&Attributes::FOLLOW_RANGE)
    }

    /// Vanilla `Sensor.isEntityAttackableIgnoringLineOfSight`.
    #[must_use]
    pub fn is_entity_attackable_ignoring_line_of_sight(
        body: &dyn AbstractPiglin,
        target: &dyn EntityBase,
    ) -> bool {
        let world = body.get_entity().world.load();
        TargetPredicate::create_attackable()
            .set_base_max_distance(Self::follow_range(body))
            .ignore_visibility()
            .ignore_distance_scaling_factor()
            .test(&world, Some(body), target)
    }

    /// The `NEAREST_VISIBLE_ATTACKABLE_PLAYER` memory of `NearestPlayerSensor`.
    #[must_use]
    pub fn nearest_visible_attackable_player(body: &dyn AbstractPiglin) -> Option<Arc<Player>> {
        let entity = body.get_entity();
        let world = entity.world.load();
        let pos = entity.pos.load();
        let range = Self::follow_range(body);
        let mut players = world.get_nearby_players(pos, range);
        sort_by_distance(&mut players, &pos);
        let targetable = TargetPredicate::create_non_attackable().set_base_max_distance(range);
        let attackable = TargetPredicate::create_attackable().set_base_max_distance(range);
        players.into_iter().find(|player| {
            targetable.test(&world, Some(body), player.as_ref())
                && attackable.test(&world, Some(body), player.as_ref())
        })
    }

    /// The `NEARBY_ADULT_PIGLINS` memory: adult piglins and brutes within the follow range.
    #[must_use]
    pub fn nearby_adult_piglins(body: &dyn AbstractPiglin) -> Vec<Arc<dyn EntityBase>> {
        let entity = body.get_entity();
        let range = Self::follow_range(body);
        let mut piglins: Vec<_> = entity
            .world
            .load()
            .get_entities_at_box(&entity.bounding_box.load().expand(range, range, range))
            .into_iter()
            .filter(|other| {
                other.get_entity().entity_id != entity.entity_id
                    && other.get_entity().is_alive()
                    && as_abstract_piglin(other.as_ref())
                        .is_some_and(AbstractPiglin::is_adult_piglin)
            })
            .collect();
        sort_by_distance(&mut piglins, &entity.pos.load());
        piglins
    }

    /// Vanilla `PiglinAi.findNearestValidAttackTarget` up to the universal anger: the target
    /// the piglin's anger points at.
    #[must_use]
    pub fn find_anger_target(body: &dyn AbstractPiglin) -> Option<Arc<dyn EntityBase>> {
        let anger = body.piglin_anger();
        let world = body.get_entity().world.load();
        if let Some(angry_at) = anger
            .angry_at()
            .and_then(|uuid| find_by_uuid(&world, uuid))
            .filter(|target| target.get_living_entity().is_some())
            && Self::is_entity_attackable_ignoring_line_of_sight(body, angry_at.as_ref())
        {
            return Some(angry_at);
        }
        if anger.has_universal_anger() {
            return Self::nearest_visible_attackable_player(body)
                .map(|player| player as Arc<dyn EntityBase>);
        }
        None
    }

    /// Vanilla `PiglinAi.angerNearbyPiglins`: a player opened or broke something piglins guard.
    pub fn anger_nearby_piglins(
        world: &World,
        player: &Arc<Player>,
        only_if_they_see_the_player: bool,
    ) {
        let universal_anger = world.level_info.load().game_rules.universal_anger;
        let search_box = player.get_entity().bounding_box.load().expand(
            Self::PLAYER_ANGER_RANGE,
            Self::PLAYER_ANGER_RANGE,
            Self::PLAYER_ANGER_RANGE,
        );
        for entity in world.get_entities_at_box(&search_box) {
            let Some(piglin) = entity.cast_any().downcast_ref::<PiglinEntity>() else {
                continue;
            };
            if !piglin.is_idle()
                || (only_if_they_see_the_player && !Self::is_entity_targetable(piglin, player))
            {
                continue;
            }
            if universal_anger {
                Self::set_anger_target_to_nearest_targetable_player_if_found(
                    piglin,
                    player.as_ref(),
                );
            } else {
                Self::set_anger_target(piglin, player.as_ref());
            }
        }
    }

    /// Vanilla `PiglinAi.maybeRetaliate`.
    pub fn maybe_retaliate(body: &dyn AbstractPiglin, attacker: &dyn EntityBase, avoiding: bool) {
        if avoiding || !Self::is_entity_attackable_ignoring_line_of_sight(body, attacker) {
            return;
        }
        if Self::is_other_target_much_further_away_than_current_attack_target(body, attacker) {
            return;
        }
        let world = body.get_entity().world.load();
        if attacker.get_player().is_some() && world.level_info.load().game_rules.universal_anger {
            Self::set_anger_target_to_nearest_targetable_player_if_found(body, attacker);
            Self::broadcast_universal_anger(body);
        } else {
            Self::set_anger_target(body, attacker);
            Self::broadcast_anger_target(body, attacker);
        }
    }

    /// Vanilla `BehaviorUtils.isOtherTargetMuchFurtherAwayThanCurrentAttackTarget` with 4 blocks.
    fn is_other_target_much_further_away_than_current_attack_target(
        body: &dyn AbstractPiglin,
        other: &dyn EntityBase,
    ) -> bool {
        const HOW_MUCH_FURTHER_AWAY: f64 = 4.0;
        let Some(current) = body.get_mob_entity().get_target() else {
            return false;
        };
        let pos = body.get_entity().pos.load();
        let to_current = pos.squared_distance_to_vec(&current.get_entity().pos.load());
        let to_other = pos.squared_distance_to_vec(&other.get_entity().pos.load());
        to_other > to_current + HOW_MUCH_FURTHER_AWAY * HOW_MUCH_FURTHER_AWAY
    }

    /// Vanilla `PiglinAi.broadcastAngerTarget`.
    pub fn broadcast_anger_target(body: &dyn AbstractPiglin, target: &dyn EntityBase) {
        let hoglin_huntable = target
            .cast_any()
            .downcast_ref::<HoglinEntity>()
            .map(HoglinEntity::can_be_hunted);
        for other in Self::nearby_adult_piglins(body) {
            let Some(piglin) = as_abstract_piglin(other.as_ref()) else {
                continue;
            };
            if hoglin_huntable.is_some_and(|huntable| !piglin.can_hunt() || !huntable) {
                continue;
            }
            Self::set_anger_target_if_closer_than_current(piglin, target);
        }
    }

    /// Vanilla `PiglinAi.broadcastUniversalAnger`.
    fn broadcast_universal_anger(body: &dyn AbstractPiglin) {
        for other in Self::nearby_adult_piglins(body) {
            if let Some(piglin) = as_abstract_piglin(other.as_ref())
                && let Some(player) = Self::nearest_visible_attackable_player(piglin)
            {
                Self::set_anger_target(piglin, player.as_ref());
            }
        }
    }

    /// Vanilla `PiglinAi.setAngerTarget`.
    pub fn set_anger_target(body: &dyn AbstractPiglin, target: &dyn EntityBase) {
        if !Self::is_entity_attackable_ignoring_line_of_sight(body, target) {
            return;
        }
        let target_entity = target.get_entity();
        body.piglin_anger().set_angry_at(target_entity.entity_uuid);
        if target_entity.entity_type == &EntityType::HOGLIN && body.can_hunt() {
            body.dont_kill_any_more_hoglins_for_a_while();
        }
        if target.get_player().is_some()
            && body
                .get_entity()
                .world
                .load()
                .level_info
                .load()
                .game_rules
                .universal_anger
        {
            body.piglin_anger().set_universal_anger();
        }
    }

    fn set_anger_target_to_nearest_targetable_player_if_found(
        body: &dyn AbstractPiglin,
        target_if_no_player_found: &dyn EntityBase,
    ) {
        match Self::nearest_visible_attackable_player(body) {
            Some(player) => Self::set_anger_target(body, player.as_ref()),
            None => Self::set_anger_target(body, target_if_no_player_found),
        }
    }

    /// Vanilla `PiglinAi.setAngerTargetIfCloserThanCurrent`.
    fn set_anger_target_if_closer_than_current(
        body: &dyn AbstractPiglin,
        new_target: &dyn EntityBase,
    ) {
        let world = body.get_entity().world.load();
        let current = body
            .piglin_anger()
            .angry_at()
            .and_then(|uuid| find_by_uuid(&world, uuid))
            .filter(|current| current.get_living_entity().is_some());
        let Some(current) = current else {
            Self::set_anger_target(body, new_target);
            return;
        };
        // Vanilla `BehaviorUtils.getTargetNearestMe`: the new target only wins when closer.
        let pos = body.get_entity().pos.load();
        let to_current = pos.squared_distance_to_vec(&current.get_entity().pos.load());
        let to_new = pos.squared_distance_to_vec(&new_target.get_entity().pos.load());
        if to_new < to_current {
            Self::set_anger_target(body, new_target);
        }
    }
}

/// Targets what the piglin is angry at.
///
/// The `ANGRY_AT` and `UNIVERSAL_ANGER` part of vanilla `StartAttacking` with
/// `findNearestValidAttackTarget`, and `StopAttackingIfTargetInvalid` once the anger points
/// elsewhere.
#[derive(Default)]
pub struct PiglinAngerTargetGoal {
    target: Option<Arc<dyn EntityBase>>,
}

impl PiglinAngerTargetGoal {
    fn anger_target(mob: &dyn Mob) -> Option<Arc<dyn EntityBase>> {
        let piglin = as_abstract_piglin(mob)?;
        if !piglin.is_adult_piglin() {
            return None;
        }
        PiglinAi::find_anger_target(piglin)
    }

    fn is_current_target(mob: &dyn Mob, target: &Arc<dyn EntityBase>) -> bool {
        mob.get_mob_entity()
            .get_target()
            .is_some_and(|current| current.get_entity().entity_id == target.get_entity().entity_id)
    }
}

impl Goal for PiglinAngerTargetGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        if !as_abstract_piglin(mob).is_some_and(AbstractPiglin::can_start_attacking) {
            return false;
        }
        self.target = Self::anger_target(mob);
        self.target.is_some()
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        let Some(target) = &self.target else {
            return false;
        };
        Self::is_current_target(mob, target)
            && Self::anger_target(mob)
                .is_some_and(|anger| anger.get_entity().entity_id == target.get_entity().entity_id)
    }

    fn start(&mut self, mob: &dyn Mob) {
        mob.set_mob_target(self.target.clone(), TargetReason::Angry);
    }

    fn stop(&mut self, mob: &dyn Mob) {
        if let Some(target) = self.target.take()
            && Self::is_current_target(mob, &target)
        {
            mob.set_mob_target(None, TargetReason::ForgotTarget);
        }
    }

    fn controls(&self) -> Controls {
        Controls::TARGET
    }
}
