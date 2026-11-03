use std::sync::Arc;
use std::sync::atomic::Ordering;

use pumpkin_data::block_properties::ComposterLikeProperties;
use pumpkin_data::data_component_impl::{CompostableImpl, EquipmentSlot};
use pumpkin_data::entity::EntityType;
use pumpkin_data::environment_attribute::Activity;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::villager::VillagerProfession;
use pumpkin_data::world::WorldEvent;
use pumpkin_data::{Block, BlockStateId};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::random::gameplay::game_random;
use pumpkin_world::world::BlockFlags;

use super::inventory::{add_item, remove_item_type};
use super::meet::{WalkDestination, WalkTarget};
use super::{VillagerEntity, held_job_site};
use crate::block::blocks::composter::ComposterBlock;
use crate::block::blocks::plant::crop::is_crop_at_max_age;
use crate::entity::ai::brain::behavior::shuffling_list::ShufflingList;
use crate::entity::ai::util::default_random_pos;
use crate::entity::item::ItemEntity;
use crate::entity::mob::Mob;
use crate::entity::{Entity, EntityBase};
use crate::world::World;
use crate::world::game_event::GameEventContext;

/// Vanilla `Villager.registerBrainGoals`' `speedModifier`.
const SPEED_MODIFIER: f64 = 0.5;
/// Vanilla `VillagerGoalPackages.STROLL_SPEED_MODIFIER`.
const STROLL_SPEED_MODIFIER: f64 = 0.4;
/// `Behavior`'s default `minDuration` and `maxDuration`, which the work behaviors keep.
const DEFAULT_BEHAVIOR_DURATION: i64 = 60;
/// Vanilla `WorkAtPoi.CHECK_COOLDOWN` and `DISTANCE`.
const WORK_AT_POI_CHECK_COOLDOWN: i64 = 300;
const WORK_AT_POI_DISTANCE: f64 = 1.73;
/// The `StrollAroundPoi.create(JOB_SITE, 0.4F, 4)` of the work package.
const JOB_SITE_STROLL_MAX_DISTANCE: f64 = 4.0;
/// The `StrollToPoi.create(JOB_SITE, 0.4F, 1, 10)` of the work package and its cooldown.
const STROLL_TO_POI_MAX_DISTANCE: f64 = 10.0;
const STROLL_TO_POI_COOLDOWN: i64 = 80;
/// The `StrollToPoiList.create(SECONDARY_JOB_SITE, speedModifier, 1, 6, JOB_SITE)` of the work
/// package and its cooldown.
const STROLL_TO_POI_LIST_MAX_DISTANCE: f64 = 6.0;
const STROLL_TO_POI_LIST_COOLDOWN: i64 = 100;
/// The `SetWalkTargetFromBlockMemory.create(JOB_SITE, speedModifier, 9, 100, 1200)` of the work
/// package, and `SetWalkTargetFromBlockMemory`'s `MAX_TRIES`.
const JOB_SITE_CLOSE_ENOUGH_DIST: i32 = 9;
const JOB_SITE_TOO_FAR_DISTANCE: i32 = 100;
const SET_WALK_TARGET_MAX_TRIES: i32 = 1000;
/// Vanilla `HarvestFarmland.HARVEST_DURATION` and `SPEED_MODIFIER`.
const HARVEST_DURATION: i32 = 200;
const HARVEST_SPEED_MODIFIER: f64 = 0.5;
/// Vanilla `UseBonemeal.BONEMEALING_DURATION`.
const BONEMEALING_DURATION: i32 = 80;
/// Vanilla `SecondaryPoiSensor.SCAN_RATE` and its search box.
pub(super) const SECONDARY_POI_SCAN_RATE: i32 = 40;
const SECONDARY_POI_HORIZONTAL_SEARCH: i32 = 4;
const SECONDARY_POI_VERTICAL_SEARCH: i32 = 2;
/// Vanilla `WorkAtComposter.COMPOSTABLE_ITEMS`.
const COMPOSTABLE_ITEMS: [&Item; 2] = [&Item::WHEAT_SEEDS, &Item::BEETROOT_SEEDS];
/// Vanilla `ComposterBlock`'s full level and the level that waits to turn into bone meal.
const COMPOSTER_READY: u8 = 8;
const COMPOSTER_FULL: u8 = 7;

#[derive(Clone, Copy, PartialEq, Eq)]
enum WorkBehavior {
    WorkAtPoi,
    StrollAroundPoi,
    StrollToPoi,
    StrollToPoiList,
    HarvestFarmland,
    UseBonemeal,
}

/// The behaviors of the work `RunOne` that run for a while, with their end time.
#[derive(Clone, Copy)]
enum Running {
    WorkAtPoi(i64),
    HarvestFarmland(i64),
    UseBonemeal(i64),
}

/// Vanilla `HarvestFarmland`'s state.
#[derive(Default)]
struct HarvestFarmland {
    above_farmland_pos: Option<BlockPos>,
    next_ok_start_time: i64,
    time_worked_so_far: i32,
    valid_farmland_around_villager: Vec<BlockPos>,
}

/// Vanilla `UseBonemeal`'s state.
#[derive(Default)]
struct UseBonemeal {
    next_work_cycle_time: i64,
    last_bonemealing_session: i32,
    time_worked_so_far: i32,
    crop_pos: Option<BlockPos>,
}

/// The state of the behaviors in vanilla `VillagerGoalPackages.getWorkPackage`.
#[derive(Default)]
pub(super) struct WorkPackage {
    running: Option<Running>,
    work_at_poi_last_check: i64,
    stroll_around_poi_next_ok_start: i64,
    stroll_to_poi_next_ok_start: i64,
    stroll_to_poi_list_next_ok_start: i64,
    harvest: HarvestFarmland,
    bonemeal: UseBonemeal,
}

const fn manhattan(a: BlockPos, b: BlockPos) -> i32 {
    (a.0.x - b.0.x).abs() + (a.0.y - b.0.y).abs() + (a.0.z - b.0.z).abs()
}

/// Vanilla `Vec3i.closerToCenterThan`.
fn closer_to_center_than(pos: BlockPos, position: Vector3<f64>, distance: f64) -> bool {
    pos.to_centered_f64().squared_distance_to_vec(&position) < distance * distance
}

fn around(center: BlockPos) -> impl Iterator<Item = BlockPos> {
    (-1..=1).flat_map(move |x| {
        (-1..=1).flat_map(move |y| (-1..=1).map(move |z| center.offset(Vector3::new(x, y, z))))
    })
}

impl VillagerEntity {
    fn set_walk_and_look_target(&self, pos: BlockPos, speed: f64, close_enough_dist: i32) {
        self.look_at_position(pos.to_centered_f64());
        self.set_walk_target(Some(WalkTarget {
            destination: WalkDestination::Pos(pos.to_centered_f64()),
            speed,
            close_enough_dist,
        }));
    }

    fn secondary_job_sites(&self) -> Vec<BlockPos> {
        self.secondary_job_sites
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Vanilla `SecondaryPoiSensor`: the profession's secondary POI blocks around the villager.
    pub(super) fn tick_secondary_poi_sensor(&self, world: &World) {
        if self
            .secondary_poi_sensor_countdown
            .fetch_sub(1, Ordering::Relaxed)
            > 1
        {
            return;
        }
        self.secondary_poi_sensor_countdown
            .store(SECONDARY_POI_SCAN_RATE, Ordering::Relaxed);
        let secondary_poi = self.profession().secondary_poi();
        let center = self.get_entity().block_pos.load();
        let mut job_sites = Vec::new();
        for x in -SECONDARY_POI_HORIZONTAL_SEARCH..=SECONDARY_POI_HORIZONTAL_SEARCH {
            for y in -SECONDARY_POI_VERTICAL_SEARCH..=SECONDARY_POI_VERTICAL_SEARCH {
                for z in -SECONDARY_POI_HORIZONTAL_SEARCH..=SECONDARY_POI_HORIZONTAL_SEARCH {
                    let pos = center.offset(Vector3::new(x, y, z));
                    if secondary_poi.contains(&world.get_block(&pos)) {
                        job_sites.push(pos);
                    }
                }
            }
        }
        *self
            .secondary_job_sites
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = job_sites;
    }

    /// The `SetWalkTargetFromBlockMemory` for `JOB_SITE` of the work package.
    fn walk_to_job_site(&self) {
        if self.walk_target().is_some() {
            return;
        }
        let Some(job_site) = self.get_job_site() else {
            return;
        };
        let body = self.get_entity().block_pos.load();
        if manhattan(job_site, body) > JOB_SITE_TOO_FAR_DISTANCE {
            let bottom_center = Vector3::new(
                f64::from(job_site.0.x) + 0.5,
                f64::from(job_site.0.y),
                f64::from(job_site.0.z) + 0.5,
            );
            let towards = (0..SET_WALK_TARGET_MAX_TRIES).find_map(|_| {
                default_random_pos::get_pos_towards(
                    self,
                    15,
                    7,
                    bottom_center,
                    std::f64::consts::FRAC_PI_2,
                )
                .filter(|pos| {
                    manhattan(BlockPos::floored_v(*pos), body) <= JOB_SITE_TOO_FAR_DISTANCE
                })
            });
            let Some(towards) = towards else {
                let pos = self
                    .job_site
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take();
                if let Some(pos) = pos {
                    Self::release_poi(
                        &self.get_entity().world.load(),
                        pos,
                        held_job_site(self.profession()),
                    );
                }
                return;
            };
            self.set_walk_target(Some(WalkTarget {
                destination: WalkDestination::Pos(towards),
                speed: SPEED_MODIFIER,
                close_enough_dist: JOB_SITE_CLOSE_ENOUGH_DIST,
            }));
        } else if manhattan(job_site, body) > JOB_SITE_CLOSE_ENOUGH_DIST {
            self.set_walk_target(Some(WalkTarget {
                destination: WalkDestination::Pos(job_site.to_centered_f64()),
                speed: SPEED_MODIFIER,
                close_enough_dist: JOB_SITE_CLOSE_ENOUGH_DIST,
            }));
        }
    }

    /// The WORK activity of vanilla `VillagerGoalPackages.getWorkPackage` besides showing
    /// trades: walking to the job site, then a `RunOne` of `WorkAtPoi` (`WorkAtComposter` for
    /// farmers), `StrollAroundPoi`, `StrollToPoi`, `StrollToPoiList`, `HarvestFarmland` and
    /// `UseBonemeal`.
    pub(super) fn tick_work(
        &self,
        world: &Arc<World>,
        game_time: i64,
        day: i64,
        activity: Activity,
    ) {
        if activity == Activity::Work {
            self.walk_to_job_site();
        }
        let mut package = self
            .work
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // A running behavior goes on after the activity changes, like in vanilla's brain.
        if package.running.is_some() {
            self.tick_or_stop_work(&mut package, world, game_time);
            return;
        }
        if activity != Activity::Work {
            return;
        }

        let farmer = self.profession() == VillagerProfession::Farmer;
        let mut behaviors = ShufflingList::new();
        behaviors.add(WorkBehavior::WorkAtPoi, 7);
        behaviors.add(WorkBehavior::StrollAroundPoi, 2);
        behaviors.add(WorkBehavior::StrollToPoi, 5);
        behaviors.add(WorkBehavior::StrollToPoiList, 5);
        behaviors.add(WorkBehavior::HarvestFarmland, if farmer { 2 } else { 5 });
        behaviors.add(WorkBehavior::UseBonemeal, if farmer { 4 } else { 7 });
        behaviors.shuffle(&mut self.get_random());
        for &behavior in behaviors.iter() {
            if self.try_start_work(&mut package, behavior, world, game_time, day) {
                break;
            }
        }
        if package.running.is_some() {
            self.tick_or_stop_work(&mut package, world, game_time);
        }
    }

    fn try_start_work(
        &self,
        package: &mut WorkPackage,
        behavior: WorkBehavior,
        world: &Arc<World>,
        game_time: i64,
        day: i64,
    ) -> bool {
        let end = game_time + DEFAULT_BEHAVIOR_DURATION;
        match behavior {
            WorkBehavior::WorkAtPoi => {
                if !self.can_start_work_at_poi(package, game_time) {
                    return false;
                }
                self.start_work_at_poi(world, game_time, day);
                package.running = Some(Running::WorkAtPoi(end));
                true
            }
            WorkBehavior::StrollAroundPoi => self.get_job_site().is_some_and(|job_site| {
                self.stroll_around_poi(
                    job_site,
                    JOB_SITE_STROLL_MAX_DISTANCE,
                    &mut package.stroll_around_poi_next_ok_start,
                    game_time,
                )
            }),
            WorkBehavior::StrollToPoi => self.stroll_to_poi(package, game_time),
            WorkBehavior::StrollToPoiList => self.stroll_to_poi_list(package, game_time),
            WorkBehavior::HarvestFarmland => {
                if self.look_target().is_some()
                    || self.walk_target().is_some()
                    || self.secondary_job_sites().is_empty()
                    || !self.can_start_harvest(&mut package.harvest, world)
                {
                    return false;
                }
                let harvest = &package.harvest;
                if game_time > harvest.next_ok_start_time
                    && let Some(pos) = harvest.above_farmland_pos
                {
                    self.set_walk_and_look_target(pos, HARVEST_SPEED_MODIFIER, 1);
                }
                package.running = Some(Running::HarvestFarmland(end));
                true
            }
            WorkBehavior::UseBonemeal => {
                if self.look_target().is_some()
                    || self.walk_target().is_some()
                    || !self.can_start_bonemeal(&mut package.bonemeal, world)
                {
                    return false;
                }
                let bonemeal = &mut package.bonemeal;
                self.set_current_crop_as_target(bonemeal);
                self.mob_entity.set_item_slot(
                    &EquipmentSlot::MAIN_HAND,
                    ItemStack::new(1, &Item::BONE_MEAL),
                );
                bonemeal.next_work_cycle_time = game_time;
                bonemeal.time_worked_so_far = 0;
                package.running = Some(Running::UseBonemeal(end));
                true
            }
        }
    }

    /// Vanilla `Behavior.tickOrStop` for the running work behavior.
    fn tick_or_stop_work(&self, package: &mut WorkPackage, world: &Arc<World>, game_time: i64) {
        let Some(running) = package.running else {
            return;
        };
        let pos = self.get_entity().pos.load();
        match running {
            Running::WorkAtPoi(end) => {
                let can_still_use = self.get_job_site().is_some_and(|job_site| {
                    closer_to_center_than(job_site, pos, WORK_AT_POI_DISTANCE)
                });
                if game_time > end || !can_still_use {
                    package.running = None;
                }
            }
            Running::HarvestFarmland(end) => {
                if game_time <= end && package.harvest.time_worked_so_far < HARVEST_DURATION {
                    self.tick_harvest(&mut package.harvest, world, game_time);
                } else {
                    // Vanilla `HarvestFarmland.stop`.
                    self.set_walk_target(None);
                    package.harvest.time_worked_so_far = 0;
                    package.harvest.next_ok_start_time = game_time + 40;
                    package.running = None;
                }
            }
            Running::UseBonemeal(end) => {
                let bonemeal = &mut package.bonemeal;
                if game_time <= end
                    && bonemeal.time_worked_so_far < BONEMEALING_DURATION
                    && bonemeal.crop_pos.is_some()
                {
                    self.tick_bonemeal(bonemeal, world, game_time);
                } else {
                    // Vanilla `UseBonemeal.stop`.
                    self.mob_entity
                        .set_item_slot(&EquipmentSlot::MAIN_HAND, ItemStack::EMPTY.clone());
                    bonemeal.last_bonemealing_session =
                        self.get_entity().tick_count.load(Ordering::Relaxed);
                    package.running = None;
                }
            }
        }
    }

    /// Vanilla `WorkAtPoi.checkExtraStartConditions`.
    fn can_start_work_at_poi(&self, package: &mut WorkPackage, game_time: i64) -> bool {
        let Some(job_site) = self.get_job_site() else {
            return false;
        };
        if game_time - package.work_at_poi_last_check < WORK_AT_POI_CHECK_COOLDOWN
            || game_random().random_range(0..2) != 0
        {
            return false;
        }
        package.work_at_poi_last_check = game_time;
        closer_to_center_than(job_site, self.get_entity().pos.load(), WORK_AT_POI_DISTANCE)
    }

    /// Vanilla `WorkAtComposter.useWorkstation`.
    pub(super) fn work_at_composter(&self, world: &Arc<World>, job_site: BlockPos) {
        let (block, state) = world.get_block_and_state_id(&job_site);
        if block != &Block::COMPOSTER {
            return;
        }
        self.make_bread();
        self.compost_items(world, job_site, block, state);
    }

    /// Vanilla `WorkAtComposter.makeBread`.
    fn make_bread(&self) {
        let mut inventory = self
            .inventory
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let count = |item: &Item| -> i32 {
            inventory
                .iter()
                .filter(|stack| stack.item == item)
                .map(|stack| i32::from(stack.item_count))
                .sum()
        };
        if count(&Item::BREAD) > 36 {
            return;
        }
        let bread = (count(&Item::WHEAT) / 3).min(3) as u8;
        if bread == 0 {
            return;
        }
        remove_item_type(&mut inventory, &Item::WHEAT, bread * 3);
        let mut bread = ItemStack::new(bread, &Item::BREAD);
        add_item(&mut inventory, &mut bread);
        drop(inventory);
        if !bread.is_empty() {
            // Vanilla `Entity.spawnAtLocation(level, stack, 0.5F)`.
            let entity = self.get_entity();
            let world = entity.world.load_full();
            let item = ItemEntity::new(
                Entity::new(
                    world.clone(),
                    entity.pos.load().add_raw(0.0, 0.5, 0.0),
                    &EntityType::ITEM,
                ),
                bread,
            );
            world.spawn_entity(Arc::new(item));
        }
    }

    /// Vanilla `WorkAtComposter.compostItems`: fills the composter with seeds beyond the ten of
    /// each kind the villager keeps, at most twenty a time.
    fn compost_items(
        &self,
        world: &Arc<World>,
        pos: BlockPos,
        block: &Block,
        mut state: BlockStateId,
    ) {
        let composter = ComposterBlock;
        if ComposterLikeProperties::from_state_id(state).level == COMPOSTER_READY {
            composter.clear_composter(world, &pos, state, block);
            state = world.get_block_state_id(&pos);
        }
        let original = state;
        let mut total_to_use: i32 = 20;
        let mut seen = [0i32; COMPOSTABLE_ITEMS.len()];
        let mut inventory = self
            .inventory
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        'slots: for stack in inventory.iter_mut().rev() {
            if total_to_use <= 0 {
                break;
            }
            let Some(index) = COMPOSTABLE_ITEMS
                .iter()
                .position(|item| *item == stack.item)
            else {
                continue;
            };
            let count = i32::from(stack.item_count);
            seen[index] += count;
            let to_use = (seen[index] - 10).min(total_to_use).min(count);
            if to_use <= 0 {
                continue;
            }
            total_to_use -= to_use;
            for _ in 0..to_use {
                state = Self::insert_into_composter(world, pos, block, state, stack);
                if ComposterLikeProperties::from_state_id(state).level == COMPOSTER_FULL {
                    break 'slots;
                }
            }
        }
        for stack in inventory.iter_mut() {
            if stack.is_empty() {
                *stack = ItemStack::EMPTY.clone();
            }
        }
        drop(inventory);
        world.sync_world_event(WorldEvent::ComposterFill, pos, i32::from(state != original));
    }

    /// Vanilla `ComposterBlock.insertItem`.
    fn insert_into_composter(
        world: &Arc<World>,
        pos: BlockPos,
        block: &Block,
        state: BlockStateId,
        stack: &mut ItemStack,
    ) -> BlockStateId {
        let level = ComposterLikeProperties::from_state_id(state).level;
        let Some(chance) = stack
            .get_data_component::<CompostableImpl>()
            .map(|compostable| compostable.chance)
        else {
            return state;
        };
        if level >= COMPOSTER_FULL {
            return state;
        }
        stack.decrement(1);
        // Vanilla `ComposterBlock.addLayer`.
        if level != 0 && !game_random().random_bool(f64::from(chance)) {
            return state;
        }
        ComposterBlock.update_level_composter(world, &pos, state, block, level + 1);
        world.get_block_state_id(&pos)
    }

    /// The `StrollToPoi` for `JOB_SITE` of the work package.
    fn stroll_to_poi(&self, package: &mut WorkPackage, game_time: i64) -> bool {
        let Some(job_site) = self.get_job_site() else {
            return false;
        };
        if !closer_to_center_than(
            job_site,
            self.get_entity().pos.load(),
            STROLL_TO_POI_MAX_DISTANCE,
        ) {
            return false;
        }
        if game_time > package.stroll_to_poi_next_ok_start {
            self.set_walk_target(Some(WalkTarget {
                destination: WalkDestination::Pos(job_site.to_centered_f64()),
                speed: STROLL_SPEED_MODIFIER,
                close_enough_dist: 1,
            }));
            package.stroll_to_poi_next_ok_start = game_time + STROLL_TO_POI_COOLDOWN;
        }
        true
    }

    /// The `StrollToPoiList` for `SECONDARY_JOB_SITE` of the work package.
    fn stroll_to_poi_list(&self, package: &mut WorkPackage, game_time: i64) -> bool {
        let Some(job_site) = self.get_job_site() else {
            return false;
        };
        let stroll_to = self.secondary_job_sites();
        if stroll_to.is_empty() {
            return false;
        }
        let target = stroll_to[game_random().random_range(0..stroll_to.len())];
        if !closer_to_center_than(
            job_site,
            self.get_entity().pos.load(),
            STROLL_TO_POI_LIST_MAX_DISTANCE,
        ) {
            return false;
        }
        if game_time > package.stroll_to_poi_list_next_ok_start {
            self.set_walk_target(Some(WalkTarget {
                destination: WalkDestination::Pos(target.to_centered_f64()),
                speed: SPEED_MODIFIER,
                close_enough_dist: 1,
            }));
            package.stroll_to_poi_list_next_ok_start = game_time + STROLL_TO_POI_LIST_COOLDOWN;
        }
        true
    }

    /// Vanilla `HarvestFarmland.validPos`.
    fn is_harvestable(world: &World, pos: BlockPos) -> bool {
        let (block, state) = world.get_block_and_state_id(&pos);
        is_crop_at_max_age(block, state) == Some(true)
            || (world.get_block_state(&pos).is_air()
                && world.get_block(&pos.down()) == &Block::FARMLAND)
    }

    /// Vanilla `HarvestFarmland.checkExtraStartConditions`.
    fn can_start_harvest(&self, harvest: &mut HarvestFarmland, world: &World) -> bool {
        if !world.level_info.load().game_rules.mob_griefing
            || self.profession() != VillagerProfession::Farmer
        {
            return false;
        }
        harvest.valid_farmland_around_villager = around(self.get_entity().block_pos.load())
            .filter(|pos| Self::is_harvestable(world, *pos))
            .collect();
        harvest.above_farmland_pos = Self::pick_farmland(harvest);
        harvest.above_farmland_pos.is_some()
    }

    /// Vanilla `HarvestFarmland.getValidFarmland`.
    fn pick_farmland(harvest: &HarvestFarmland) -> Option<BlockPos> {
        let valid = &harvest.valid_farmland_around_villager;
        (!valid.is_empty()).then(|| valid[game_random().random_range(0..valid.len())])
    }

    /// Vanilla `HarvestFarmland.tick`: breaks the mature crop it stands on, replants farmland
    /// from its seeds, and moves on to the next spot.
    fn tick_harvest(&self, harvest: &mut HarvestFarmland, world: &Arc<World>, game_time: i64) {
        let position = self.get_entity().pos.load();
        if harvest
            .above_farmland_pos
            .is_some_and(|pos| !closer_to_center_than(pos, position, 1.0))
        {
            return;
        }
        if let Some(pos) = harvest.above_farmland_pos
            && game_time > harvest.next_ok_start_time
        {
            self.look_at_position(pos.to_centered_f64());
            let (block, state_id) = world.get_block_and_state_id(&pos);
            let is_air = world.get_block_state(&pos).is_air();
            let on_farmland = world.get_block(&pos.down()) == &Block::FARMLAND;
            let crop_maturity = is_crop_at_max_age(block, state_id);
            if crop_maturity == Some(true) {
                world.break_block(&pos, None, BlockFlags::NOTIFY_ALL);
            }
            if is_air && on_farmland {
                self.plant_seed(world, pos);
            }
            if crop_maturity == Some(false) {
                harvest
                    .valid_farmland_around_villager
                    .retain(|farmland| *farmland != pos);
                harvest.above_farmland_pos = Self::pick_farmland(harvest);
                if let Some(next) = harvest.above_farmland_pos {
                    harvest.next_ok_start_time = game_time + 20;
                    self.set_walk_and_look_target(next, HARVEST_SPEED_MODIFIER, 1);
                }
            }
        }
        harvest.time_worked_so_far += 1;
    }

    /// The replanting of vanilla `HarvestFarmland.tick`.
    fn plant_seed(&self, world: &Arc<World>, pos: BlockPos) {
        let mut inventory = self
            .inventory
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some((stack, place)) = inventory.iter_mut().find_map(|stack| {
            if stack.is_empty()
                || !stack
                    .item
                    .has_tag(&tag::Item::MINECRAFT_VILLAGER_PLANTABLE_SEEDS)
            {
                return None;
            }
            let block = Block::from_item_id(stack.item.id)?;
            Some((stack, block))
        }) else {
            return;
        };
        world.set_block_state(&pos, place.default_state.id, BlockFlags::NOTIFY_ALL);
        world.emit_game_event_with_context(
            GameEvent::BlockPlace.name(),
            pos.to_centered_f64(),
            &GameEventContext {
                source_entity: Some(self),
            },
        );
        world.play_sound_fine(
            Sound::ItemCropPlant,
            SoundCategory::Blocks,
            &pos.0.to_f64(),
            1.0,
            1.0,
        );
        stack.decrement(1);
        if stack.is_empty() {
            *stack = ItemStack::EMPTY.clone();
        }
    }

    /// Vanilla `UseBonemeal.checkExtraStartConditions`.
    fn can_start_bonemeal(&self, bonemeal: &mut UseBonemeal, world: &World) -> bool {
        let tick_count = self.get_entity().tick_count.load(Ordering::Relaxed);
        if tick_count % 10 != 0
            || (bonemeal.last_bonemealing_session != 0
                && bonemeal.last_bonemealing_session + 160 > tick_count)
            || self.count_item(&Item::BONE_MEAL) <= 0
        {
            return false;
        }
        bonemeal.crop_pos = self.pick_bonemeal_target(world);
        bonemeal.crop_pos.is_some()
    }

    /// Vanilla `UseBonemeal.pickNextTarget`: a random growing crop around the villager.
    fn pick_bonemeal_target(&self, world: &World) -> Option<BlockPos> {
        let mut result = None;
        let mut count = 0;
        for pos in around(self.get_entity().block_pos.load()) {
            let (block, state) = world.get_block_and_state_id(&pos);
            if is_crop_at_max_age(block, state) == Some(false) {
                count += 1;
                if game_random().random_range(0..count) == 0 {
                    result = Some(pos);
                }
            }
        }
        result
    }

    /// Vanilla `UseBonemeal.setCurrentCropAsTarget`.
    fn set_current_crop_as_target(&self, bonemeal: &UseBonemeal) {
        if let Some(pos) = bonemeal.crop_pos {
            self.set_walk_and_look_target(pos, SPEED_MODIFIER, 1);
        }
    }

    /// Vanilla `UseBonemeal.tick`.
    fn tick_bonemeal(&self, bonemeal: &mut UseBonemeal, world: &Arc<World>, game_time: i64) {
        let Some(target) = bonemeal.crop_pos else {
            return;
        };
        if game_time < bonemeal.next_work_cycle_time
            || !closer_to_center_than(target, self.get_entity().pos.load(), 1.0)
        {
            return;
        }
        if self.grow_crop(world, target) {
            world.sync_world_event(WorldEvent::ParticlesAndSoundPlantGrowth, target, 15);
            bonemeal.crop_pos = self.pick_bonemeal_target(world);
            self.set_current_crop_as_target(bonemeal);
            bonemeal.next_work_cycle_time = game_time + 40;
        }
        bonemeal.time_worked_so_far += 1;
    }

    /// Vanilla `BoneMealItem.growCrop` with the first bone meal of the inventory.
    fn grow_crop(&self, world: &Arc<World>, pos: BlockPos) -> bool {
        let Some(server) = world.server.upgrade() else {
            return false;
        };
        let mut inventory = self
            .inventory
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(stack) = inventory
            .iter_mut()
            .find(|stack| stack.item == &Item::BONE_MEAL)
        else {
            return false;
        };
        let (block, state) = world.get_block_and_state_id(&pos);
        if !server.block_registry.bone_meal(block, world, &pos, state) {
            return false;
        }
        stack.decrement(1);
        if stack.is_empty() {
            *stack = ItemStack::EMPTY.clone();
        }
        true
    }
}
