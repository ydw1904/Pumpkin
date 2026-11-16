use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::poi_type::PoiType;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::tracked_data;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::poi::Occupancy;

use crate::entity::EntityBase;
use crate::entity::ai::behavior::fleeing::Flee;
use crate::entity::ai::goal::open_door::OpenDoorGoal;
use crate::entity::ai::goal::revenge::RevengeGoal;
use crate::entity::ai::goal::{Controls, Goal};
use crate::entity::ai::pathfinder::path::Path;
use crate::entity::ai::target_match::TargetMatch;
use crate::entity::ai::util::default_random_pos;
use crate::entity::item::ItemEntity;
use crate::entity::living::LivingEntity;
use crate::entity::mob::Mob;
use crate::entity::mob::patrol::{PatrolData, PatrollingMonster};

/// Vanilla `Raider.defineSynchedData`.
pub fn define_raider_synched_data(entity: &crate::entity::Entity) {
    entity
        .synched_data
        .define(tracked_data::raider::IS_CELEBRATING, false);
}

#[must_use]
pub fn is_raider(entity_type: &'static EntityType) -> bool {
    entity_type.has_tag(&tag::EntityType::MINECRAFT_RAIDERS)
}

/// Vanilla `AbstractIllager.considersEntityAsAlly`: teammates, and `illager_friends` while
/// neither side is on a team.
#[must_use]
pub fn illager_considers_entity_as_ally(illager: &dyn EntityBase, other: &dyn EntityBase) -> bool {
    illager.is_on_same_team(other)
        || (other
            .get_entity()
            .entity_type
            .has_tag(&tag::EntityType::MINECRAFT_ILLAGER_FRIENDS)
            && illager.get_team_name().is_none()
            && other.get_team_name().is_none())
}

/// Vanilla `HurtByTargetGoal(this, Raider.class).setAlertOthers()`.
#[must_use]
pub fn revenge_goal() -> Box<RevengeGoal> {
    Box::new(RevengeGoal::new(true).ignoring(is_raider).alerting_others())
}

/// Illagers keep 8 blocks from a creaking.
#[must_use]
pub const fn flee_creaking(priority: u8, walk_speed: f64, sprint_speed: f64) -> Flee {
    Flee::new(
        priority,
        TargetMatch::Type(&EntityType::CREAKING),
        8.0,
        walk_speed,
        sprint_speed,
    )
}

/// Vanilla `Raid.getOminousBannerInstance`.
#[must_use]
pub fn create_ominous_banner() -> ItemStack {
    use pumpkin_data::data_component::DataComponent;
    use pumpkin_data::data_component_impl::{
        BannerPatternLayer, BannerPatternsImpl, DataComponentImpl, ItemNameImpl, Rarity,
        RarityImpl, TooltipDisplayImpl,
    };
    use pumpkin_data::dye_color::DyeColor;
    let layers = [
        ("minecraft:rhombus", DyeColor::Cyan),
        ("minecraft:stripe_bottom", DyeColor::LightGray),
        ("minecraft:stripe_center", DyeColor::Gray),
        ("minecraft:border", DyeColor::LightGray),
        ("minecraft:stripe_middle", DyeColor::Black),
        ("minecraft:half_horizontal", DyeColor::LightGray),
        ("minecraft:circle", DyeColor::LightGray),
        ("minecraft:border", DyeColor::Black),
    ]
    .into_iter()
    .map(|(pattern, color)| BannerPatternLayer {
        pattern: pattern.to_string(),
        color,
    })
    .collect();
    let mut stack = ItemStack::new(1, &Item::WHITE_BANNER);
    stack.patch.extend([
        (
            DataComponent::BannerPatterns,
            Some(BannerPatternsImpl { layers }.to_dyn()),
        ),
        (
            DataComponent::TooltipDisplay,
            Some(
                TooltipDisplayImpl {
                    hide_tooltip: false,
                    hidden_components: std::borrow::Cow::Borrowed(&[DataComponent::BannerPatterns]),
                }
                .to_dyn(),
            ),
        ),
        (
            DataComponent::ItemName,
            Some(
                ItemNameImpl {
                    name: "block.minecraft.ominous_banner".into(),
                }
                .to_dyn(),
            ),
        ),
        (
            DataComponent::Rarity,
            Some(
                RarityImpl {
                    rarity: Rarity::Uncommon,
                }
                .to_dyn(),
            ),
        ),
    ]);
    stack
}

#[must_use]
pub const fn is_ominous_banner(stack: &ItemStack) -> bool {
    stack.item.id == Item::WHITE_BANNER.id
}

pub struct RaiderData {
    pub patrol_data: PatrolData,
    pub wave: AtomicI32,
    pub can_join_raid: AtomicBool,
    pub ticks_outside_raid: AtomicI32,
    pub is_celebrating: AtomicBool,
    pub raid_id: AtomicCell<Option<i32>>,
}

impl Default for RaiderData {
    fn default() -> Self {
        Self {
            patrol_data: PatrolData::default(),
            wave: AtomicI32::new(0),
            can_join_raid: AtomicBool::new(false),
            ticks_outside_raid: AtomicI32::new(0),
            is_celebrating: AtomicBool::new(false),
            raid_id: AtomicCell::new(None),
        }
    }
}

pub trait Raider: PatrollingMonster {
    fn get_raider_data(&self) -> &RaiderData;

    fn can_join_raid(&self) -> bool {
        self.get_raider_data().can_join_raid.load(Ordering::Relaxed)
    }

    fn set_can_join_raid(&self, can_join: bool) {
        self.get_raider_data()
            .can_join_raid
            .store(can_join, Ordering::Relaxed);
    }

    fn get_wave(&self) -> i32 {
        self.get_raider_data().wave.load(Ordering::Relaxed)
    }

    fn set_wave(&self, wave: i32) {
        self.get_raider_data().wave.store(wave, Ordering::Relaxed);
    }

    fn is_celebrating(&self) -> bool {
        self.get_raider_data()
            .is_celebrating
            .load(Ordering::Relaxed)
    }

    fn set_celebrating(&self, celebrating: bool) {
        self.get_raider_data()
            .is_celebrating
            .store(celebrating, Ordering::Relaxed);
        let entity = &self.get_mob_entity().living_entity.entity;
        entity.set_synced_data(tracked_data::pillager::IS_CELEBRATING, celebrating);
    }

    fn has_active_raid(&self) -> bool {
        self.get_raider_data().raid_id.load().is_some()
    }

    /// Vanilla `Raider.isCaptain`: a patrol leader wearing the ominous banner.
    fn is_captain(&self) -> bool {
        self.is_patrol_leader()
            && crate::entity::mob::equipment::item_in(self.get_mob_entity(), &EquipmentSlot::HEAD)
                .are_equal(&create_ominous_banner())
    }

    fn get_celebrate_sound(&self) -> Sound;

    /// Vanilla `Raider.applyRaidBuffs`, for a raider the raid spawned in `wave`.
    fn apply_raid_buffs(&self, _raid: &crate::world::raid::Raid, _wave: i32, _is_captain: bool) {}

    fn write_raider_nbt(&self, nbt: &mut NbtCompound) {
        self.write_patrol_nbt(nbt);
        let data = self.get_raider_data();
        nbt.put_int("Wave", data.wave.load(Ordering::Relaxed));
        nbt.put_bool("CanJoinRaid", data.can_join_raid.load(Ordering::Relaxed));
        if let Some(raid_id) = data.raid_id.load() {
            nbt.put_int("RaidId", raid_id);
        }
    }

    fn read_raider_nbt(&self, nbt: &NbtCompound) {
        self.read_patrol_nbt(nbt);
        let data = self.get_raider_data();
        if let Some(wave) = nbt.get_int("Wave") {
            data.wave.store(wave, Ordering::Relaxed);
        }
        if let Some(can_join) = nbt.get_bool("CanJoinRaid") {
            data.can_join_raid.store(can_join, Ordering::Relaxed);
        }
        if let Some(raid_id) = nbt.get_int("RaidId") {
            data.raid_id.store(Some(raid_id));
            // Vanilla rejoins the raid here; it happens at the next raid tick.
            let entity = &self.get_mob_entity().living_entity.entity;
            entity
                .world
                .load()
                .raids
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .pending_members
                .push(crate::world::raid::PendingRaidMember {
                    raid_id,
                    wave: data.wave.load(Ordering::Relaxed),
                    uuid: entity.entity_uuid,
                    leader: self.is_patrol_leader(),
                });
        }
    }
}

/// Vanilla `Raider.HoldGroundAttackGoal`: a patrol that spots a target stops and calls the
/// raiders around it, and charges once the target comes within the hostile radius.
pub struct HoldGroundAttackGoal {
    hostile_radius_sqr: f64,
}

impl HoldGroundAttackGoal {
    /// Vanilla `shoutTargeting`: non-combat targeting of raiders within 8 blocks.
    const SHOUT_RANGE: f64 = 8.0;

    #[must_use]
    pub fn new(hostile_radius: f32) -> Self {
        Self {
            hostile_radius_sqr: f64::from(hostile_radius * hostile_radius),
        }
    }

    /// Vanilla `getNearbyEntities(Raider.class, shoutTargeting, mob, box.inflate(8))`.
    fn shout_targets(mob: &dyn Mob) -> Vec<Arc<dyn EntityBase>> {
        let entity = mob.get_entity();
        let pos = entity.pos.load();
        let world = entity.world.load();
        world
            .get_entities_of_types_at_box(
                &entity.bounding_box.load().expand(
                    Self::SHOUT_RANGE,
                    Self::SHOUT_RANGE,
                    Self::SHOUT_RANGE,
                ),
                is_raider,
            )
            .into_iter()
            .filter(|other| {
                let other_entity = other.get_entity();
                other_entity.entity_id != entity.entity_id
                    && other
                        .get_living_entity()
                        .is_some_and(LivingEntity::is_alive)
                    && other_entity.pos.load().squared_distance_to_vec(&pos)
                        <= Self::SHOUT_RANGE * Self::SHOUT_RANGE
            })
            .collect()
    }
}

impl Goal for HoldGroundAttackGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(raider) = mob.as_raider() else {
            return false;
        };
        let mob_entity = mob.get_mob_entity();
        let hurt_by_player = mob_entity
            .living_entity
            .get_last_hurt_by_mob()
            .is_some_and(|attacker| attacker.get_entity().entity_type == &EntityType::PLAYER);
        raider.get_raider_data().raid_id.load().is_none()
            && raider.is_patrolling()
            && mob_entity.get_target().is_some()
            && !mob_entity.is_attacking()
            && !hurt_by_player
    }

    fn start(&mut self, mob: &dyn Mob) {
        mob.get_mob_entity().steered().navigator().stop();
        let target = mob.get_mob_entity().get_target();
        for other in Self::shout_targets(mob) {
            if let Some(other_mob) = other.get_mob() {
                other_mob.get_mob_entity().set_target(target.clone());
            }
        }
    }

    fn stop(&mut self, mob: &dyn Mob) {
        let Some(target) = mob.get_mob_entity().get_target() else {
            return;
        };
        for other in Self::shout_targets(mob) {
            if let Some(other_mob) = other.get_mob() {
                other_mob.get_mob_entity().set_target(Some(target.clone()));
                other_mob.get_mob_entity().set_attacking(true);
            }
        }
        mob.get_mob_entity().set_attacking(true);
    }

    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let Some(target) = mob.get_mob_entity().get_target() else {
            return;
        };
        let dist_sq = mob
            .get_entity()
            .pos
            .load()
            .squared_distance_to_vec(&target.get_entity().pos.load());
        if dist_sq > self.hostile_radius_sqr {
            mob.get_mob_entity()
                .look_control
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .look_at_entity_with_range(&target, 30.0, 30.0);
            if mob.get_random().random_range(0..50) == 0 {
                mob.play_ambient_sound();
            }
        } else {
            mob.get_mob_entity().set_attacking(true);
        }
    }
}

/// Vanilla `Raider.ALLOWED_ITEMS`: ominous banners that can be picked up.
fn is_allowed_banner(item: &ItemEntity) -> bool {
    !item.has_pickup_delay()
        && item.get_entity().is_alive()
        && item
            .get_item_stack()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .are_equal(&create_ominous_banner())
}

/// Vanilla `Raider.pickUpItem`: an ominous banner makes the raider the captain of its wave when
/// its raid has none; anything else is picked up like any mob would.
pub fn pick_up_item(raider: &dyn Raider, item: &ItemEntity) {
    let stack = item
        .get_item_stack()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let world = raider.get_entity().world.load_full();
    let wave = raider.get_wave();
    let leaderless_raid = raider.get_raider_data().raid_id.load().filter(|&id| {
        world
            .raids
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(id)
            .is_some_and(|raid| raid.is_active() && !raid.group_to_leader_map.contains_key(&wave))
    });
    let Some(raid_id) = leaderless_raid.filter(|_| stack.are_equal(&create_ominous_banner()))
    else {
        crate::entity::mob::equipment::pick_up_item_as_equipment(raider, item);
        return;
    };
    let mob_entity = raider.get_mob_entity();
    let living = &mob_entity.living_entity;
    let slot = EquipmentSlot::HEAD;
    let current = crate::entity::mob::equipment::item_in(mob_entity, &slot);
    if !current.is_empty()
        && (raider.get_random().random::<f32>() - 0.1).max(0.0) < mob_entity.drop_chance(&slot)
    {
        living.entity.spawn_at_location(current);
    }
    living.on_item_pickup(item);
    mob_entity.set_item_slot(&slot, stack.clone());
    living.pickup(item.get_entity(), u32::from(stack.item_count));
    item.get_entity().remove();
    if let Some(raid) = world
        .raids
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get_mut(raid_id)
    {
        raid.set_leader(wave, raider);
    }
    raider.set_patrol_leader(true);
}

/// Vanilla `cannotPickUpBanner`: a raider takes a banner only in an unfinished raid whose wave
/// has no living captain, and only without a banner of its own.
fn cannot_pick_up_banner(raider: &dyn Raider) -> bool {
    let Some(raid_id) = raider.get_raider_data().raid_id.load() else {
        return true;
    };
    if !raider.can_be_leader()
        || crate::entity::mob::equipment::item_in(raider.get_mob_entity(), &EquipmentSlot::HEAD)
            .are_equal(&create_ominous_banner())
    {
        return true;
    }
    let world = raider.get_entity().world.load();
    let leader = {
        let raids = world
            .raids
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(raid) = raids.get(raid_id).filter(|raid| raid.is_active()) else {
            return true;
        };
        if raid.is_over() {
            return true;
        }
        raid.group_to_leader_map.get(&raider.get_wave()).copied()
    };
    leader
        .and_then(|uuid| world.get_entity_by_uuid(uuid))
        .is_some_and(|leader| {
            leader
                .get_living_entity()
                .is_some_and(LivingEntity::is_alive)
        })
}

/// Vanilla `Raider.ObtainRaidLeaderBannerGoal`: a raider whose wave lost its captain walks to a
/// dropped ominous banner it can reach and puts it on.
#[derive(Default)]
pub struct ObtainRaidLeaderBannerGoal {
    /// Banner entity ids that couldn't be reached, with the game time they are tried again.
    unreachable_banner_cache: HashMap<i32, i64>,
    path: Option<Path>,
    pursued_banner: Option<Arc<dyn EntityBase>>,
}

impl ObtainRaidLeaderBannerGoal {
    const UNREACHABLE_RETRY_TICKS: i64 = 600;
    const PICKUP_DISTANCE: f64 = 1.414;
}

impl Goal for ObtainRaidLeaderBannerGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(raider) = mob.as_raider() else {
            return false;
        };
        if cannot_pick_up_banner(raider) {
            return false;
        }
        let entity = mob.get_entity();
        let world = entity.world.load();
        let game_time = world
            .level_time
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .query_gametime();
        let follow_range = mob
            .get_mob_entity()
            .living_entity
            .get_attribute_value(&Attributes::FOLLOW_RANGE);
        let area = entity
            .bounding_box
            .load()
            .expand(follow_range, 8.0, follow_range);
        let mut temp_cache = HashMap::new();
        for banner in world.get_entities_of_types_at_box(&area, |t| t == &EntityType::ITEM) {
            if !banner.get_item_entity().is_some_and(is_allowed_banner) {
                continue;
            }
            let id = banner.get_entity().entity_id;
            let unreachable_until = self
                .unreachable_banner_cache
                .get(&id)
                .copied()
                .unwrap_or(i64::MIN);
            if game_time < unreachable_until {
                temp_cache.insert(id, unreachable_until);
                continue;
            }
            let steered = mob.get_mob_entity().steered();
            let path = steered.navigator().create_path(
                &steered.mob_entity().living_entity,
                banner.get_entity().block_pos.load().0.to_f64(),
                1,
            );
            if let Some(path) = path.filter(Path::can_reach) {
                self.path = Some(path);
                self.pursued_banner = Some(banner);
                return true;
            }
            temp_cache.insert(id, game_time + Self::UNREACHABLE_RETRY_TICKS);
        }
        self.unreachable_banner_cache = temp_cache;
        false
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        let Some(banner) = &self.pursued_banner else {
            return false;
        };
        if banner.get_entity().is_removed() || mob.get_mob_entity().steered().navigator().is_done()
        {
            return false;
        }
        mob.as_raider()
            .is_some_and(|raider| !cannot_pick_up_banner(raider))
    }

    fn start(&mut self, mob: &dyn Mob) {
        let steered = mob.get_mob_entity().steered();
        steered.navigator().move_to_path(
            self.path.take(),
            1.15,
            &steered.mob_entity().living_entity,
        );
    }

    fn stop(&mut self, _mob: &dyn Mob) {
        self.path = None;
        self.pursued_banner = None;
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let (Some(banner), Some(raider)) = (&self.pursued_banner, mob.as_raider()) else {
            return;
        };
        let close = banner
            .get_entity()
            .pos
            .load()
            .squared_distance_to_vec(&mob.get_entity().pos.load())
            < Self::PICKUP_DISTANCE * Self::PICKUP_DISTANCE;
        if close && let Some(item) = banner.get_item_entity() {
            pick_up_item(raider, item);
        }
    }
}

/// Vanilla `Raider.RaiderCelebration`: raiders cheer and jump around once their raid is lost.
pub struct RaiderCelebrationGoal;

impl Goal for RaiderCelebrationGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(raid_id) = mob
            .as_raider()
            .and_then(|raider| raider.get_raider_data().raid_id.load())
        else {
            return false;
        };
        if !mob.get_mob_entity().living_entity.is_alive()
            || mob.get_mob_entity().get_target().is_some()
        {
            return false;
        }
        mob.get_entity()
            .world
            .load()
            .raids
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(raid_id)
            .is_some_and(crate::world::raid::Raid::is_loss)
    }

    fn start(&mut self, mob: &dyn Mob) {
        if let Some(raider) = mob.as_raider() {
            raider.set_celebrating(true);
        }
    }

    fn stop(&mut self, mob: &dyn Mob) {
        if let Some(raider) = mob.as_raider() {
            raider.set_celebrating(false);
        }
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let Some(raider) = mob.as_raider() else {
            return;
        };
        let entity = mob.get_entity();
        if !entity.silent.load(Ordering::Relaxed)
            && mob.get_random().random_range(0..self.get_tick_count(100)) == 0
        {
            mob.make_sound(raider.get_celebrate_sound());
        }
        if !entity.has_vehicle() && mob.get_random().random_range(0..self.get_tick_count(50)) == 0 {
            mob.get_mob_entity()
                .jump_control
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .jump();
        }
    }
}

/// Vanilla `Raider.RaiderMoveThroughVillageGoal`: during a raid, raiders walk from one village
/// home to another, remembering the last few they reached.
pub struct RaiderMoveThroughVillageGoal {
    speed_modifier: f64,
    poi_pos: BlockPos,
    visited: Vec<BlockPos>,
    stuck: bool,
}

impl RaiderMoveThroughVillageGoal {
    /// Vanilla `distanceToPoi` for raiders.
    const DISTANCE_TO_POI: f64 = 1.0;
    const POI_SEARCH_RADIUS: i32 = 48;
    /// Vanilla forgets the oldest home once it remembers more than this many.
    const MAX_VISITED: usize = 2;

    #[must_use]
    pub const fn new(speed_modifier: f64) -> Self {
        Self {
            speed_modifier,
            poi_pos: BlockPos::ZERO,
            visited: Vec::new(),
            stuck: false,
        }
    }

    fn is_valid_raid(mob: &dyn Mob) -> bool {
        let Some(raid_id) = mob
            .as_raider()
            .and_then(|raider| raider.get_raider_data().raid_id.load())
        else {
            return false;
        };
        mob.get_entity()
            .world
            .load()
            .raids
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(raid_id)
            .is_some_and(|raid| raid.is_active() && !raid.is_over())
    }

    fn poi_closer_than(&self, mob: &dyn Mob, distance: f64) -> bool {
        self.poi_pos
            .to_centered_f64()
            .squared_distance_to_vec(&mob.get_entity().pos.load())
            < distance * distance
    }
}

impl Goal for RaiderMoveThroughVillageGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        if self.visited.len() > Self::MAX_VISITED {
            self.visited.remove(0);
        }
        if !Self::is_valid_raid(mob) {
            return false;
        }
        let world = mob.get_entity().world.load();
        let home = world
            .portal_poi
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_random(
                &|poi_type| poi_type == PoiType::Home,
                &|pos| !self.visited.contains(&pos),
                Occupancy::Any,
                mob.get_entity().block_pos.load(),
                Self::POI_SEARCH_RADIUS,
                &mut mob.get_random(),
            );
        let Some(home) = home else {
            return false;
        };
        self.poi_pos = home;
        mob.get_mob_entity().get_target().is_none()
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        if mob.get_mob_entity().steered().navigator().is_done() {
            return false;
        }
        let width = f64::from(mob.get_entity().width());
        mob.get_mob_entity().get_target().is_none()
            && !self.poi_closer_than(mob, width + Self::DISTANCE_TO_POI)
            && !self.stuck
    }

    fn start(&mut self, mob: &dyn Mob) {
        let mob_entity = mob.get_mob_entity();
        mob_entity
            .living_entity
            .no_action_time
            .store(0, Ordering::Relaxed);
        let steered = mob_entity.steered();
        steered.navigator().move_to_coords(
            f64::from(self.poi_pos.0.x),
            f64::from(self.poi_pos.0.y),
            f64::from(self.poi_pos.0.z),
            self.speed_modifier,
            &steered.mob_entity().living_entity,
        );
        self.stuck = false;
    }

    fn stop(&mut self, mob: &dyn Mob) {
        if self.poi_closer_than(mob, Self::DISTANCE_TO_POI) {
            self.visited.push(self.poi_pos);
        }
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }

    fn tick(&mut self, mob: &dyn Mob) {
        if !mob.get_mob_entity().steered().navigator().is_done() {
            return;
        }
        let poi = Vector3::new(
            f64::from(self.poi_pos.0.x) + 0.5,
            f64::from(self.poi_pos.0.y),
            f64::from(self.poi_pos.0.z) + 0.5,
        );
        let next =
            default_random_pos::get_pos_towards(mob, 16, 7, poi, std::f64::consts::PI / 10.0)
                .or_else(|| {
                    default_random_pos::get_pos_towards(mob, 8, 7, poi, std::f64::consts::FRAC_PI_2)
                });
        let Some(next) = next else {
            self.stuck = true;
            return;
        };
        let steered = mob.get_mob_entity().steered();
        steered.navigator().move_to_coords(
            next.x,
            next.y,
            next.z,
            self.speed_modifier,
            &steered.mob_entity().living_entity,
        );
    }
}

pub use crate::entity::ai::goal::pathfind_to_raid::PathfindToRaidGoal;

/// Vanilla `AbstractIllager.RaiderOpenDoorGoal`: opens doors, only during a raid.
pub struct RaiderOpenDoorGoal {
    open_door: OpenDoorGoal,
}

impl Default for RaiderOpenDoorGoal {
    fn default() -> Self {
        Self {
            open_door: OpenDoorGoal::new(false),
        }
    }
}

impl Goal for RaiderOpenDoorGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        self.open_door.can_start(mob) && mob.as_raider().is_some_and(Raider::has_active_raid)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.open_door.should_continue(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.open_door.start(mob);
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.open_door.stop(mob);
    }

    fn tick(&mut self, mob: &dyn Mob) {
        self.open_door.tick(mob);
    }

    fn should_run_every_tick(&self) -> bool {
        self.open_door.should_run_every_tick()
    }

    fn controls(&self) -> Controls {
        self.open_door.controls()
    }
}
