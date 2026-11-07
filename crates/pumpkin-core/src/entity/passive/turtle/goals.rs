use std::sync::{Weak, atomic::Ordering};

use pumpkin_data::block_properties::TurtleEggLikeProperties;
use pumpkin_data::entity::EntityType;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::world::BlockFlags;

use super::{TurtleEntity, as_turtle};
use crate::block::blocks::turtle_egg::TurtleEggBlock;
use crate::entity::ai::goal::{
    Controls, Goal, GoalReach, ParentHandle,
    breed::BreedGoal,
    escape_danger::EscapeDangerGoal,
    goal_selector::GoalSelector,
    look_at_entity::LookAtEntityGoal,
    move_to_target_pos::{MoveToTargetPos, MoveToTargetPosGoal},
    tempt::TemptGoal,
    to_goal_ticks,
    wander_around::WanderAroundGoal,
};
use crate::entity::ai::pathfinder::NavigatorGoal;
use crate::entity::ai::util::default_random_pos;
use crate::entity::experience_orb::ExperienceOrbEntity;
use crate::entity::mob::Mob;
use crate::entity::{EntityBase, ageable::AgeableMob};
use crate::world::World;
use crate::world::game_event::GameEventContext;

/// The `EntityBreedEvent` child of a turtle breeding, which lays an egg instead.
const NO_CHILD_ID: i32 = 0;
/// Vanilla `Turtle.TurtleRandomStrollGoal` interval.
const RANDOM_STROLL_INTERVAL: i32 = 100;
/// Vanilla `TurtleLayEggGoal`: the turtle lays only this close to home.
const LAY_EGG_HOME_DISTANCE: f64 = 9.0;
/// Ticks of digging before the egg is laid.
const LAY_EGG_TICKS: i32 = 200;
/// Love ticks a turtle gets after laying its egg.
const IN_LOVE_TIME_AFTER_LAYING: i32 = 600;
/// Vanilla `TurtleGoToWaterGoal.GIVE_UP_TICKS`.
const GO_TO_WATER_GIVE_UP_TICKS: i32 = 1200;
/// Vanilla `TurtleGoHomeGoal.GIVE_UP_TICKS`.
const GO_HOME_GIVE_UP_TICKS: i32 = 600;
const GO_HOME_CHANCE: i32 = 700;
const FAR_FROM_HOME_DISTANCE: f64 = 64.0;
const HOME_REACHED_DISTANCE: f64 = 7.0;
const CLOSE_TO_HOME_DISTANCE: f64 = 16.0;
const TRAVEL_XZ_DISTANCE: i32 = 512;
const TRAVEL_Y_DISTANCE: i32 = 4;
/// Radius around the next travel step that must be loaded.
const TRAVEL_LOADED_RADIUS: i32 = 34;
const BREEDING_COOLDOWN: i32 = 6000;
/// Search range of `TurtleGoToWaterGoal`, beyond the navigation's own range.
const GO_TO_WATER_SEARCH_RANGE: i32 = 24;
const LAY_EGG_SEARCH_RANGE: i32 = 16;

pub(super) fn register(selector: &GoalSelector, mob_weak: Weak<dyn Mob>) {
    selector.add_goal(0, EscapeDangerGoal::turtle(1.2));
    selector.add_goal(
        1,
        Box::new(TurtleBreedGoal(BreedGoal::with_breed(1.0, turtle_breed))),
    );
    selector.add_goal(1, TurtleLayEggGoal::new(1.0));
    selector.add_goal(
        2,
        Box::new(TemptGoal::with_tag(
            1.1,
            &tag::Item::MINECRAFT_TURTLE_FOOD,
            false,
        )),
    );
    // Vanilla picks 2.0 for babies, but registers the goal before the turtle can be one.
    selector.add_goal(3, TurtleGoToWaterGoal::new(1.0));
    selector.add_goal(4, Box::new(TurtleGoHomeGoal::new(1.0)));
    selector.add_goal(7, Box::new(TurtleTravelGoal::new(1.0)));
    selector.add_goal(
        8,
        LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 8.0),
    );
    selector.add_goal(
        9,
        Box::new(
            WanderAroundGoal::with_interval(1.0, RANDOM_STROLL_INTERVAL)
                .gated_by(can_random_stroll),
        ),
    );
}

/// Vanilla `TurtleRandomStrollGoal.canUse`.
fn can_random_stroll(mob: &dyn Mob) -> bool {
    !mob.get_entity().is_in_water()
        && as_turtle(mob).is_some_and(|turtle| !turtle.is_going_home() && !turtle.has_egg())
}

fn move_to(mob: &dyn Mob, target: Vector3<f64>, speed: f64) {
    let pos = mob.get_entity().pos.load();
    mob.get_mob_entity()
        .steered()
        .navigator()
        .set_progress(NavigatorGoal::new(pos, target, speed));
}

fn navigation_done(mob: &dyn Mob) -> bool {
    mob.get_mob_entity().steered().navigator().is_done()
}

/// Vanilla `TurtleBreedGoal.breed`: the turtle gets an egg instead of a baby.
fn turtle_breed(mob: &dyn Mob, mate: &dyn EntityBase) {
    // Plugins see the breeding like other animals', with no child yet.
    let experience = mob.approve_breeding(
        mate.get_entity().entity_id,
        NO_CHILD_ID,
        mob.get_random().random_range(1..8),
    );
    // Cancelled: love mode still ends with the usual cooldown, as for other animals.
    if experience.is_some() {
        let (first, second) = BreedGoal::love_causes(mob, mate);
        BreedGoal::award_love_cause(mob, first.or(second));
        if let Some(turtle) = as_turtle(mob) {
            turtle.set_has_egg(true);
        }
    }
    let mob_entity = mob.get_mob_entity();
    mob_entity
        .breeding_cooldown
        .store(BREEDING_COOLDOWN, Ordering::Relaxed);
    mate.set_breeding_cooldown(BREEDING_COOLDOWN);
    mob_entity.reset_love_ticks();
    mate.reset_love();
    let entity = mob.get_entity();
    let world = entity.world.load_full();
    if world.level_info.load().game_rules.mob_drops
        && let Some(Ok(experience)) = experience.map(u32::try_from)
        && experience > 0
    {
        ExperienceOrbEntity::spawn(&world, entity.pos.load(), experience);
    }
}

/// Vanilla `Turtle.TurtleBreedGoal`.
struct TurtleBreedGoal(Box<BreedGoal>);

impl Goal for TurtleBreedGoal {
    fn tick_reach(&self, mob: &dyn Mob, running: bool) -> Option<GoalReach> {
        self.0.tick_reach(mob, running)
    }

    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        self.0.can_start(mob) && as_turtle(mob).is_some_and(|turtle| !turtle.has_egg())
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.0.should_continue(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.0.start(mob);
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.0.stop(mob);
    }

    fn tick(&mut self, mob: &dyn Mob) {
        self.0.tick(mob);
    }

    fn controls(&self) -> Controls {
        self.0.controls()
    }
}

/// How far from the body a running block goal keeps checking its target.
fn target_reads(mob: &dyn Mob, target: BlockPos) -> f64 {
    mob.get_entity()
        .pos
        .load()
        .squared_distance_to_vec(&target.to_centered_f64())
        .sqrt()
        + 2.0
}

/// Vanilla `Turtle.TurtleLayEggGoal`.
struct TurtleLayEggGoal {
    move_to_block: MoveToTargetPosGoal<Self>,
}

impl TurtleLayEggGoal {
    #[expect(
        clippy::unnecessary_box_returns,
        reason = "the inner goal points back into this heap allocation"
    )]
    fn new(speed: f64) -> Box<Self> {
        let mut this = Box::new(Self {
            move_to_block: MoveToTargetPosGoal::with_default(
                ParentHandle::none(),
                speed,
                LAY_EGG_SEARCH_RANGE,
            ),
        });
        // SAFETY: the boxed goal never moves, so the handle stays valid for the goal's lifetime.
        this.move_to_block.move_to_target_pos = unsafe { ParentHandle::new(&this) };
        this
    }

    fn may_lay(turtle: &TurtleEntity) -> bool {
        turtle.has_egg() && turtle.home_within(LAY_EGG_HOME_DISTANCE)
    }

    fn lay_egg(&self, mob: &dyn Mob, turtle: &TurtleEntity) {
        let entity = mob.get_entity();
        let world = entity.world.load_full();
        let pitch = 0.9 + mob.get_random().random::<f32>() * 0.2;
        world.play_sound_fine(
            Sound::EntityTurtleLayEgg,
            SoundCategory::Blocks,
            &entity.block_pos.load().to_centered_f64(),
            0.3,
            pitch,
        );
        let egg_pos = self.move_to_block.target_pos.up();
        let mut egg = TurtleEggLikeProperties::default(&pumpkin_data::Block::TURTLE_EGG);
        egg.eggs = mob.get_random().random_range(1..=4);
        world.set_block_state(
            &egg_pos,
            egg.to_state_id(&pumpkin_data::Block::TURTLE_EGG),
            BlockFlags::NOTIFY_ALL,
        );
        world.emit_game_event_with_context(
            GameEvent::BlockPlace.name(),
            egg_pos.to_centered_f64(),
            &GameEventContext {
                source_entity: Some(mob as &dyn EntityBase),
            },
        );
        turtle.set_has_egg(false);
        turtle.set_laying_egg(false);
        mob.get_mob_entity()
            .love_ticks
            .store(IN_LOVE_TIME_AFTER_LAYING, Ordering::Relaxed);
    }
}

impl Goal for TurtleLayEggGoal {
    fn tick_reach(&self, mob: &dyn Mob, running: bool) -> Option<GoalReach> {
        // A turtle that already dug long enough can start and lay in the same tick; the egg
        // goes into the block it stands in.
        let lays = as_turtle(mob).is_some_and(|turtle| {
            turtle.is_laying_egg() && turtle.lay_egg_counter.load(Ordering::Relaxed) > LAY_EGG_TICKS
        });
        Some(GoalReach {
            speed: self.move_to_block.speed,
            block_reads: if running {
                target_reads(mob, self.move_to_block.target_pos)
            } else {
                0.0
            },
            changes_blocks: lays,
            game_events: if lays { 2.0 } else { 0.0 },
            ..GoalReach::default()
        })
    }

    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        as_turtle(mob).is_some_and(Self::may_lay) && self.move_to_block.can_start(mob)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.move_to_block.should_continue(mob) && as_turtle(mob).is_some_and(Self::may_lay)
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.move_to_block.start(mob);
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.move_to_block.stop(mob);
    }

    fn tick(&mut self, mob: &dyn Mob) {
        self.move_to_block.tick(mob);
        let Some(turtle) = as_turtle(mob) else {
            return;
        };
        if mob.get_entity().is_in_water() || !self.move_to_block.is_reached_target() {
            return;
        }
        let counter = turtle.lay_egg_counter.load(Ordering::Relaxed);
        if counter < 1 {
            turtle.set_laying_egg(true);
        } else if counter > self.get_tick_count(LAY_EGG_TICKS) {
            self.lay_egg(mob, turtle);
        }
        if turtle.is_laying_egg() {
            turtle.lay_egg_counter.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn should_run_every_tick(&self) -> bool {
        self.move_to_block.should_run_every_tick()
    }

    fn controls(&self) -> Controls {
        self.move_to_block.controls()
    }
}

impl MoveToTargetPos for TurtleLayEggGoal {
    fn is_target_pos(&self, world: &World, block_pos: BlockPos) -> bool {
        world.get_block_state(&block_pos.up()).is_air()
            && TurtleEggBlock::is_sand(world, &block_pos)
    }
}

/// Vanilla `Turtle.TurtleGoToWaterGoal`.
struct TurtleGoToWaterGoal {
    move_to_block: MoveToTargetPosGoal<Self>,
}

impl TurtleGoToWaterGoal {
    #[expect(
        clippy::unnecessary_box_returns,
        reason = "the inner goal points back into this heap allocation"
    )]
    fn new(speed: f64) -> Box<Self> {
        let mut move_to_block = MoveToTargetPosGoal::with_default(
            ParentHandle::none(),
            speed,
            GO_TO_WATER_SEARCH_RANGE,
        );
        // Vanilla `verticalSearchStart = -1`: only the layer two blocks below the feet.
        move_to_block.lowest_y = -1;
        let mut this = Box::new(Self { move_to_block });
        // SAFETY: the boxed goal never moves, so the handle stays valid for the goal's lifetime.
        this.move_to_block.move_to_target_pos = unsafe { ParentHandle::new(&this) };
        this
    }
}

impl Goal for TurtleGoToWaterGoal {
    fn tick_reach(&self, mob: &dyn Mob, running: bool) -> Option<GoalReach> {
        let search = f64::from(GO_TO_WATER_SEARCH_RANGE) + 1.0;
        Some(GoalReach {
            speed: self.move_to_block.speed,
            block_reads: if running {
                target_reads(mob, self.move_to_block.target_pos).max(search)
            } else {
                search
            },
            ..GoalReach::default()
        })
    }

    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let in_water = mob.get_entity().is_in_water();
        let Some(turtle) = as_turtle(mob) else {
            return false;
        };
        if (turtle.is_baby() && !in_water)
            || (!turtle.is_going_home() && !in_water && !turtle.has_egg())
        {
            self.move_to_block.can_start(mob)
        } else {
            false
        }
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        let entity = mob.get_entity();
        !entity.is_in_water()
            && self.move_to_block.trying_time <= GO_TO_WATER_GIVE_UP_TICKS
            && self.is_target_pos(&entity.world.load(), self.move_to_block.target_pos)
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.move_to_block.start(mob);
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.move_to_block.stop(mob);
    }

    fn tick(&mut self, mob: &dyn Mob) {
        self.move_to_block.tick(mob);
    }

    fn should_run_every_tick(&self) -> bool {
        self.move_to_block.should_run_every_tick()
    }

    fn controls(&self) -> Controls {
        self.move_to_block.controls()
    }
}

impl MoveToTargetPos for TurtleGoToWaterGoal {
    fn is_target_pos(&self, world: &World, block_pos: BlockPos) -> bool {
        world.get_block(&block_pos) == &pumpkin_data::Block::WATER
    }

    fn should_recalculate_path(&self, trying_time: i32) -> bool {
        trying_time % 160 == 0
    }
}

/// The next step towards `target`, like vanilla `TurtleGoHomeGoal` and `TurtleTravelGoal`.
fn step_towards(mob: &dyn Mob, target: Vector3<f64>) -> Option<Vector3<f64>> {
    default_random_pos::get_pos_towards(mob, 16, 3, target, std::f64::consts::PI / 10.0).or_else(
        || default_random_pos::get_pos_towards(mob, 8, 7, target, std::f64::consts::FRAC_PI_2),
    )
}

/// Vanilla `Turtle.TurtleGoHomeGoal`.
struct TurtleGoHomeGoal {
    speed: f64,
    stuck: bool,
    close_to_home_try_ticks: i32,
}

impl TurtleGoHomeGoal {
    const fn new(speed: f64) -> Self {
        Self {
            speed,
            stuck: false,
            close_to_home_try_ticks: 0,
        }
    }
}

impl Goal for TurtleGoHomeGoal {
    fn tick_reach(&self, _mob: &dyn Mob, _running: bool) -> Option<GoalReach> {
        // Its steps and the block it checks stay within the navigation's search range.
        Some(GoalReach {
            speed: self.speed,
            ..GoalReach::default()
        })
    }

    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(turtle) = as_turtle(mob) else {
            return false;
        };
        if turtle.is_baby() {
            return false;
        }
        if turtle.has_egg() {
            return true;
        }
        mob.get_random()
            .random_range(0..to_goal_ticks(GO_HOME_CHANCE))
            == 0
            && !turtle.home_within(FAR_FROM_HOME_DISTANCE)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        as_turtle(mob).is_some_and(|turtle| !turtle.home_within(HOME_REACHED_DISTANCE))
            && !self.stuck
            && self.close_to_home_try_ticks <= self.get_tick_count(GO_HOME_GIVE_UP_TICKS)
    }

    fn start(&mut self, mob: &dyn Mob) {
        if let Some(turtle) = as_turtle(mob) {
            turtle.going_home.store(true, Ordering::Relaxed);
        }
        self.stuck = false;
        self.close_to_home_try_ticks = 0;
    }

    fn stop(&mut self, mob: &dyn Mob) {
        if let Some(turtle) = as_turtle(mob) {
            turtle.going_home.store(false, Ordering::Relaxed);
        }
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let Some(turtle) = as_turtle(mob) else {
            return;
        };
        let home = turtle.home_pos();
        let close_to_home = turtle.home_within(CLOSE_TO_HOME_DISTANCE);
        if close_to_home {
            self.close_to_home_try_ticks += 1;
        }
        if !navigation_done(mob) {
            return;
        }
        let home_vec = home.to_f64();
        let mut next = step_towards(mob, home_vec);
        if let Some(pos) = next
            && !close_to_home
            && mob
                .get_entity()
                .world
                .load()
                .get_block(&BlockPos::floored(pos.x, pos.y, pos.z))
                != &pumpkin_data::Block::WATER
        {
            next = default_random_pos::get_pos_towards(
                mob,
                16,
                5,
                home_vec,
                std::f64::consts::FRAC_PI_2,
            );
        }
        match next {
            Some(pos) => move_to(mob, pos, self.speed),
            None => self.stuck = true,
        }
    }

    fn controls(&self) -> Controls {
        Controls::empty()
    }
}

/// Vanilla `Turtle.TurtleTravelGoal`.
struct TurtleTravelGoal {
    speed: f64,
    stuck: bool,
}

impl TurtleTravelGoal {
    const fn new(speed: f64) -> Self {
        Self {
            speed,
            stuck: false,
        }
    }
}

impl Goal for TurtleTravelGoal {
    fn tick_reach(&self, _mob: &dyn Mob, _running: bool) -> Option<GoalReach> {
        // Its steps stay within the navigation's search range; the loaded-chunk check reads
        // no blocks.
        Some(GoalReach {
            speed: self.speed,
            ..GoalReach::default()
        })
    }

    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        mob.get_entity().is_in_water()
            && as_turtle(mob).is_some_and(|turtle| !turtle.is_going_home() && !turtle.has_egg())
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        !navigation_done(mob)
            && !self.stuck
            && !mob.get_mob_entity().is_in_love()
            && as_turtle(mob).is_some_and(|turtle| !turtle.is_going_home() && !turtle.has_egg())
    }

    fn start(&mut self, mob: &dyn Mob) {
        let (xt, mut yt, zt) = {
            let random = mob.get_random();
            (
                random.random_range(0..=2 * TRAVEL_XZ_DISTANCE) - TRAVEL_XZ_DISTANCE,
                random.random_range(0..=2 * TRAVEL_Y_DISTANCE) - TRAVEL_Y_DISTANCE,
                random.random_range(0..=2 * TRAVEL_XZ_DISTANCE) - TRAVEL_XZ_DISTANCE,
            )
        };
        let entity = mob.get_entity();
        let pos = entity.pos.load();
        if f64::from(yt) + pos.y > f64::from(entity.world.load().sea_level - 1) {
            yt = 0;
        }
        if let Some(turtle) = as_turtle(mob) {
            turtle.travel_pos.store(Some(BlockPos::floored(
                f64::from(xt) + pos.x,
                f64::from(yt) + pos.y,
                f64::from(zt) + pos.z,
            )));
        }
        self.stuck = false;
    }

    fn stop(&mut self, mob: &dyn Mob) {
        if let Some(turtle) = as_turtle(mob) {
            turtle.travel_pos.store(None);
        }
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let Some(travel_pos) = as_turtle(mob).and_then(|turtle| turtle.travel_pos.load()) else {
            self.stuck = true;
            return;
        };
        if !navigation_done(mob) {
            return;
        }
        let next = step_towards(mob, travel_pos.to_f64()).filter(|pos| {
            let x = pos.x.floor() as i32;
            let z = pos.z.floor() as i32;
            mob.get_entity().world.load().has_chunks_at(
                x - TRAVEL_LOADED_RADIUS,
                z - TRAVEL_LOADED_RADIUS,
                x + TRAVEL_LOADED_RADIUS,
                z + TRAVEL_LOADED_RADIUS,
            )
        });
        match next {
            Some(pos) => move_to(mob, pos, self.speed),
            None => self.stuck = true,
        }
    }

    fn controls(&self) -> Controls {
        Controls::empty()
    }
}
