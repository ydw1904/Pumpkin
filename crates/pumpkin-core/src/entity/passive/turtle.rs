use crate::entity::r#type::CreatureSpawnLevel;
use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, AtomicI32, Ordering},
};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::BlockState;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::damage::DamageType;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::world::WorldEvent;
use pumpkin_nbt::{compound::NbtCompound, tag::NbtTag};
use pumpkin_util::math::position::BlockPos;

use crate::block::blocks::turtle_egg::TurtleEggBlock;
use crate::entity::{
    Entity, EntityBase,
    ageable::{AgeableData, AgeableMob},
    ai::{
        control::{
            Control, MoveControlTrait,
            move_control::{MoveControl, Operation},
        },
        goal::GoalReach,
        pathfinder::{Navigator, node::PathType},
    },
    mob::{Mob, MobEntity, spawn::SpawnGroupData},
    passive::animal::Animal,
    player::Player,
};
use crate::world::World;

mod goals;

/// Vanilla `Turtle.getAmbientSoundInterval`.
const AMBIENT_SOUND_INTERVAL: i32 = 200;
const TURTLE_GROW_LOOT_TABLE: &str = "minecraft:gameplay/turtle_grow";
/// Vertical speed `TurtleMoveControl.updateSpeed` adds in water.
const WATER_BUOYANCY: f64 = 0.005;
/// Share of the speed `TurtleMoveControl.tick` turns into vertical speed.
const VERTICAL_ACCELERATION: f64 = 0.1;
/// Vanilla `Turtle.travelInWater` input scale.
const WATER_INPUT_SCALE: f64 = 0.1;
const WATER_DRAG: f64 = 0.9;
/// Within this distance of home a turtle heading home stops sinking in water.
const HOME_FLOAT_DISTANCE: f64 = 20.0;
/// Outside this distance of home a turtle swims at half speed.
const HOME_FULL_SPEED_DISTANCE: f64 = 16.0;

/// Turtle is prey for wolves and cats.
pub fn is_baby_on_land(mob: &dyn Mob) -> bool {
    mob.as_ageable().is_some_and(AgeableMob::is_baby) && !mob.get_entity().is_in_water()
}

/// The turtle behind `mob`, for the goals and controls shared with other code.
fn as_turtle(mob: &dyn Mob) -> Option<&TurtleEntity> {
    mob.cast_any().downcast_ref::<TurtleEntity>()
}

/// Vanilla `Vec3i.closerToCenterThan`.
fn closer_to_center_than(pos: BlockPos, entity: &Entity, distance: f64) -> bool {
    pos.to_centered_f64()
        .squared_distance_to_vec(&entity.pos.load())
        < distance * distance
}

pub struct TurtleEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    pub has_egg: AtomicBool,
    pub laying_egg: AtomicBool,
    lay_egg_counter: AtomicI32,
    home_pos: AtomicCell<BlockPos>,
    travel_pos: AtomicCell<Option<BlockPos>>,
    going_home: AtomicBool,
}

impl TurtleEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mut mob_entity = MobEntity::new(entity);
        let mut navigator = Navigator::amphibious(false);
        navigator.set_pathfinding_malus(PathType::Water, 0.0);
        navigator.set_pathfinding_malus(PathType::DoorIronClosed, -1.0);
        navigator.set_pathfinding_malus(PathType::DoorWoodClosed, -1.0);
        navigator.set_pathfinding_malus(PathType::DoorOpen, -1.0);
        *mob_entity
            .navigator
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = navigator;
        *mob_entity
            .move_control
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Box::new(TurtleMoveControl::default());
        let turtle = Self {
            mob_entity,
            ageable_data: AgeableData::default(),
            has_egg: AtomicBool::new(false),
            laying_egg: AtomicBool::new(false),
            lay_egg_counter: AtomicI32::new(0),
            home_pos: AtomicCell::new(BlockPos::ZERO),
            travel_pos: AtomicCell::new(None),
            going_home: AtomicBool::new(false),
        };
        let mob_arc = Arc::new(turtle);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        goals::register(&mob_arc.mob_entity.goals_selector, mob_weak);

        mob_arc
    }

    #[must_use]
    pub fn has_egg(&self) -> bool {
        self.has_egg.load(Ordering::Relaxed)
    }

    pub fn set_has_egg(&self, has_egg: bool) {
        self.has_egg.store(has_egg, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(pumpkin_data::tracked_data::turtle::HAS_EGG, has_egg);
    }

    #[must_use]
    pub fn is_laying_egg(&self) -> bool {
        self.laying_egg.load(Ordering::Relaxed)
    }

    /// Vanilla `Turtle.setLayingEgg`, which also restarts the digging counter.
    pub fn set_laying_egg(&self, laying_egg: bool) {
        self.lay_egg_counter
            .store(i32::from(laying_egg), Ordering::Relaxed);
        self.laying_egg.store(laying_egg, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(pumpkin_data::tracked_data::turtle::LAYING_EGG, laying_egg);
    }

    #[must_use]
    pub fn home_pos(&self) -> BlockPos {
        self.home_pos.load()
    }

    pub fn set_home_pos(&self, pos: BlockPos) {
        self.home_pos.store(pos);
    }

    fn is_going_home(&self) -> bool {
        self.going_home.load(Ordering::Relaxed)
    }

    fn home_within(&self, distance: f64) -> bool {
        closer_to_center_than(self.home_pos(), self.get_entity(), distance)
    }
}

impl AgeableMob for TurtleEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }

    fn age_boundary_reached(&self) {
        if !self.mob_entity.living_entity.is_baby()
            && self
                .get_entity()
                .world
                .load()
                .level_info
                .load()
                .game_rules
                .mob_drops
        {
            crate::entity::mob::drop_from_gift_loot_table(self, TURTLE_GROW_LOOT_TABLE);
        }
    }
}

impl Animal for TurtleEntity {
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack.item.has_tag(&tag::Item::MINECRAFT_TURTLE_FOOD)
    }

    fn can_fall_in_love(&self) -> bool {
        !self.get_mob_entity().is_in_love() && !self.has_egg()
    }
}

impl Mob for TurtleEntity {
    fn get_ambient_sound_interval(&self) -> i32 {
        AMBIENT_SOUND_INTERVAL
    }

    // Vanilla `Turtle.getAmbientSound` falls back to `Mob`'s silence.
    fn get_ambient_sound(&self) -> Option<Sound> {
        let entity = self.get_entity();
        (!entity.is_in_water()
            && entity.on_ground.load(Ordering::Relaxed)
            && !self.mob_entity.living_entity.is_baby())
        .then_some(Sound::EntityTurtleAmbientLand)
    }

    // The digging effects in `mob_ai_step` stay at the turtle's own block.
    fn has_audited_tick(&self) -> bool {
        true
    }

    /// `TurtleMoveControl` steers vertically before the turtle moves.
    fn audited_species_reach(&self, speed: f64) -> GoalReach {
        let current = f64::from(self.mob_entity.living_entity.get_speed());
        GoalReach {
            impulse: WATER_BUOYANCY + VERTICAL_ACCELERATION * current.max(speed),
            ..GoalReach::default()
        }
    }

    /// Vanilla `TurtlePathNavigation.isStableDestination`: a travelling turtle only stops in water.
    fn is_stable_destination(&self, world: &World, pos: &BlockPos) -> bool {
        if self.travel_pos.load().is_some() {
            world.get_block(pos) == &pumpkin_data::Block::WATER
        } else {
            !world.get_block_state(&pos.down()).is_air()
        }
    }

    /// Vanilla `Turtle.getWalkTargetValue`.
    fn get_walk_target_value(&self, level: &dyn CreatureSpawnLevel, pos: &BlockPos) -> f32 {
        if (!self.is_going_home() && level.fluid(pos).has_tag(&tag::Fluid::MINECRAFT_WATER))
            || level
                .block(&pos.down())
                .has_tag(&tag::Block::MINECRAFT_SAND)
        {
            10.0
        } else {
            level.pathfinding_cost_from_light_levels(pos)
        }
    }

    fn mob_is_pushed_by_fluids(&self) -> bool {
        false
    }

    /// Vanilla `Turtle.travelInWater`.
    fn travel_in_water(&self, caller: &dyn EntityBase, _gravity: f64, _falling: bool) {
        let living = &self.mob_entity.living_entity;
        let entity = &living.entity;
        entity.update_velocity_from_input(living.movement_input.load(), WATER_INPUT_SCALE);
        living.make_move(caller);
        let mut velocity = entity.velocity.load() * WATER_DRAG;
        let has_target = self
            .mob_entity
            .target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some();
        if !has_target && (!self.is_going_home() || !self.home_within(HOME_FLOAT_DISTANCE)) {
            velocity.y -= WATER_BUOYANCY;
        }
        entity.velocity.store(velocity);
    }

    /// Vanilla `Turtle.aiStep`: the sand particles while digging.
    fn mob_ai_step(&self) {
        let counter = self.lay_egg_counter.load(Ordering::Relaxed);
        let entity = self.get_entity();
        if entity.is_alive() && self.is_laying_egg() && counter >= 1 && counter % 5 == 0 {
            let pos = entity.block_pos.load();
            let world = entity.world.load();
            if TurtleEggBlock::on_sand(world.as_ref(), &pos) {
                let below = world.get_block_state_id(&pos.down());
                world.sync_world_event(
                    WorldEvent::ParticlesAndSoundDestroyBlock,
                    pos,
                    i32::from(below.as_u16()),
                );
                entity.emit_own_game_event(self, pumpkin_data::game_event::GameEvent::EntityAction);
            }
        }
    }

    fn finalize_spawn(
        &self,
        _world: &Arc<World>,
        _reason: crate::entity::mob::spawn::EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        self.set_home_pos(self.get_entity().block_pos.load());
        self.finalize_spawn_super(group_data)
    }

    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {
        let sound = if self.get_entity().age.load(Ordering::Relaxed) < 0 {
            Sound::EntityTurtleShambleBaby
        } else {
            Sound::EntityTurtleShamble
        };
        self.play_entity_sound(sound, 0.15, 1.0);
    }

    fn mob_play_swim_sound(&self, volume: f32) {
        self.play_default_swim_sound(volume * 1.5);
    }

    /// Vanilla `Turtle.thunderHit`: lightning always kills a turtle, without setting it alight.
    fn mob_on_lightning_strike(
        &self,
        caller: &dyn EntityBase,
        lightning: &crate::entity::lightning::LightningBoltEntity,
    ) {
        let cause = lightning.get_cause();
        caller.damage_with_context(
            caller,
            f32::MAX,
            DamageType::LIGHTNING_BOLT,
            None,
            Some(lightning),
            cause.as_deref().map(|p| p as &dyn EntityBase),
        );
    }

    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn as_animal(&self) -> Option<&dyn Animal> {
        Some(self)
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.write_ageable_nbt(nbt);
        let home = self.home_pos();
        nbt.put(
            "home_pos",
            NbtTag::IntArray(vec![home.0.x, home.0.y, home.0.z]),
        );
        nbt.put_bool("has_egg", self.has_egg());
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        let home = nbt
            .get_int_array("home_pos")
            .filter(|pos| pos.len() == 3)
            .map_or_else(
                || self.get_entity().block_pos.load(),
                |pos| BlockPos::new(pos[0], pos[1], pos[2]),
            );
        self.set_home_pos(home);
        self.read_ageable_nbt(nbt);
        self.set_has_egg(nbt.get_bool("has_egg").unwrap_or(false));
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        let is_baby = entity.age.load(Ordering::Relaxed) < 0;
        if is_baby {
            entity.set_synced_data(pumpkin_data::tracked_data::turtle::DATA_BABY_ID, true);
        }
        entity.set_synced_data(pumpkin_data::tracked_data::turtle::HAS_EGG, self.has_egg());
        entity.set_synced_data(
            pumpkin_data::tracked_data::turtle::LAYING_EGG,
            self.is_laying_egg(),
        );
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        self.animal_interact(player, item_stack)
    }
}

/// Vanilla `Turtle.TurtleMoveControl`.
#[derive(Default)]
struct TurtleMoveControl {
    inner: MoveControl,
}

impl TurtleMoveControl {
    fn update_speed(mob: &dyn Mob) {
        let mob_entity = mob.get_mob_entity();
        let living = &mob_entity.living_entity;
        let entity = &living.entity;
        if entity.is_in_water() {
            entity
                .velocity
                .store(entity.velocity.load().add_raw(0.0, WATER_BUOYANCY, 0.0));
            if as_turtle(mob).is_some_and(|turtle| !turtle.home_within(HOME_FULL_SPEED_DISTANCE)) {
                mob_entity.set_speed((living.get_speed() / 2.0).max(0.08));
            }
            if living.is_baby() {
                mob_entity.set_speed((living.get_speed() / 3.0).max(0.06));
            }
        } else if entity.on_ground.load(Ordering::Relaxed) {
            mob_entity.set_speed((living.get_speed() / 2.0).max(0.06));
        }
    }
}

impl Control for TurtleMoveControl {}

impl MoveControlTrait for TurtleMoveControl {
    fn tick(&mut self, mob: &dyn Mob) {
        Self::update_speed(mob);
        let mob_entity = mob.get_mob_entity();
        let living = &mob_entity.living_entity;
        let entity = &living.entity;
        // Unlike `MoveControl`, the wanted position stays until the path is done.
        if self.inner.operation == Operation::MoveTo && !mob_entity.steered().navigator().is_done()
        {
            let pos = entity.pos.load();
            let xd = self.inner.wanted_x - pos.x;
            let mut yd = self.inner.wanted_y - pos.y;
            let zd = self.inner.wanted_z - pos.z;
            let distance = (xd * xd + yd * yd + zd * zd).sqrt();
            if distance < f64::from(1.0E-5f32) {
                mob_entity.set_speed(0.0);
            } else {
                yd /= distance;
                let yaw = MoveControl::rotlerp(
                    entity.yaw.load(),
                    zd.atan2(xd).to_degrees() as f32 - 90.0,
                    MoveControl::MAX_TURN,
                );
                entity.yaw.store(yaw);
                entity.body_yaw.store(yaw);
                let target_speed = (self.inner.speed_modifier
                    * living.get_attribute_value(&Attributes::MOVEMENT_SPEED))
                    as f32;
                let speed = living.get_speed();
                mob_entity.set_speed(speed + 0.125 * (target_speed - speed));
                entity.velocity.store(entity.velocity.load().add_raw(
                    0.0,
                    f64::from(living.get_speed()) * yd * VERTICAL_ACCELERATION,
                    0.0,
                ));
            }
        } else {
            mob_entity.set_speed(0.0);
        }
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
