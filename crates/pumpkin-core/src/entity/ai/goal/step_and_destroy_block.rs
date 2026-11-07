use super::{Controls, Goal, GoalReach, to_goal_ticks};
use crate::entity::ai::goal::ParentHandle;
use crate::entity::ai::goal::move_to_target_pos::{MoveToTargetPos, MoveToTargetPosGoal};
use crate::entity::mob::Mob;
use crate::world::World;
use pumpkin_data::{Block, BlockStateId};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::world::BlockFlags;
use std::sync::Arc;

const MAX_COOLDOWN: i32 = 20;
/// Ticks spent standing on the block before it breaks.
const DESTROY_TICKS: i32 = 60;
/// Vertical speed of the stomping on the block.
const STEP_SPEED: f64 = 0.3;

pub struct StepAndDestroyBlockGoal<S: Stepping, M: MoveToTargetPos> {
    pub stepping: ParentHandle<S>,
    pub move_to_target_pos_goal: MoveToTargetPosGoal<M>,
    target_block: &'static Block,
    counter: i32,
}

impl<S: Stepping, M: MoveToTargetPos> StepAndDestroyBlockGoal<S, M> {
    // if stepping is set to None then will use itself for stepping
    #[must_use]
    pub fn new(
        stepping: ParentHandle<S>,
        move_to_target_pos: ParentHandle<M>,
        target_block: &'static Block,
        speed: f64,
        max_y_difference: i32,
    ) -> Self {
        let move_to_target_pos_goal =
            MoveToTargetPosGoal::new(move_to_target_pos, speed, 24, max_y_difference);

        Self {
            stepping,
            move_to_target_pos_goal,
            target_block,
            counter: 0,
        }
    }

    #[must_use]
    pub fn with_default(target_block: &'static Block, speed: f64, max_y_difference: i32) -> Self {
        Self::new(
            ParentHandle::none(),
            ParentHandle::none(),
            target_block,
            speed,
            max_y_difference,
        )
    }
    /// Whether `find_target_pos` can find the target block at all, so the search over every
    /// block in range can be skipped when no section nearby holds it.
    fn target_block_may_be_in_range(&self, mob: &dyn Mob, world: &World) -> bool {
        let goal = &self.move_to_target_pos_goal;
        let pos = mob.get_entity().block_pos.load();
        let horizontal = goal.range - 1;
        // The search steps through vertical offsets from `lowest_y` outwards up to
        // `max_y_difference`, one block below the mob.
        let min = BlockPos::new(
            pos.0.x - horizontal,
            pos.0.y - 1 + goal.lowest_y.min(-goal.max_y_difference),
            pos.0.z - horizontal,
        );
        let max = BlockPos::new(
            pos.0.x + horizontal,
            pos.0.y - 1 + goal.max_y_difference,
            pos.0.z + horizontal,
        );
        world.may_get_block_in(min, max, self.target_block)
    }

    fn tweak_to_proper_pos(&self, pos: BlockPos, world: &World) -> Option<BlockPos> {
        if world.get_block(&pos).id == self.target_block.id {
            Some(pos)
        } else {
            let block_pos = [
                pos.down(),
                pos.west(),
                pos.east(),
                pos.north(),
                pos.south(),
                pos.down().down(),
            ];

            for pos in block_pos {
                if world.get_block(&pos).id == self.target_block.id {
                    return Some(pos);
                }
            }
            None
        }
    }
}

pub trait Stepping: Send + Sync {
    fn tick_stepping(&self, _world: Arc<World>, _block_pos: BlockPos) {}

    fn on_destroy_block(&self, _world: Arc<World>, _block_pos: BlockPos) {}
}

impl<S: Stepping + Send + Sync, M: MoveToTargetPos + Send + Sync> Goal
    for StepAndDestroyBlockGoal<S, M>
{
    fn tick_reach(&self, _mob: &dyn Mob, running: bool) -> Option<GoalReach> {
        let goal = &self.move_to_target_pos_goal;
        Some(GoalReach {
            speed: goal.speed,
            // `tick` removes the block below once it has stood there long enough.
            changes_blocks: running && self.counter > DESTROY_TICKS,
            impulse: if running { STEP_SPEED } else { 0.0 },
            // `find_target_pos` looks this far around the mob's block.
            block_reads: f64::from(goal.range) + 1.0,
            ..GoalReach::default()
        })
    }

    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let world = mob.get_entity().world.load();
        let level_info = world.level_info.load();
        if !level_info.game_rules.mob_griefing {
            false
        } else if self.move_to_target_pos_goal.cooldown > 0 {
            self.move_to_target_pos_goal.cooldown -= 1;
            false
        } else if self.target_block_may_be_in_range(mob, &world)
            && self.move_to_target_pos_goal.find_target_pos(mob)
        {
            self.move_to_target_pos_goal.cooldown = to_goal_ticks(MAX_COOLDOWN);
            true
        } else {
            self.move_to_target_pos_goal.cooldown = MoveToTargetPosGoal::<M>::get_interval(mob);
            false
        }
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.move_to_target_pos_goal.should_continue(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.move_to_target_pos_goal.start(mob);
        self.counter = 0;
    }

    fn stop(&mut self, mob: &dyn Mob) {
        // Assuming fall_distance is AtomicF32/f32
        mob.get_mob_entity().living_entity.fall_distance.store(1.0);
    }

    fn tick(&mut self, mob: &dyn Mob) {
        self.move_to_target_pos_goal.tick(mob);
        let mob_entity = mob.get_mob_entity();
        let world = mob.get_entity().world.load_full();
        let block_pos = mob.get_entity().block_pos.load();

        let Some(tweak_pos) = self.tweak_to_proper_pos(block_pos, &world) else {
            return;
        };
        if !self.move_to_target_pos_goal.reached {
            return;
        }
        let counter = self.counter;

        if counter > 0 {
            let velocity = mob_entity.living_entity.entity.velocity.load();
            mob_entity
                .living_entity
                .entity
                .set_velocity(Vector3::new(velocity.x, STEP_SPEED, velocity.z));
            // TODO: spawn particles
        }

        if counter % 2 == 0 {
            let velocity = mob_entity.living_entity.entity.velocity.load();
            mob_entity.living_entity.entity.set_velocity(Vector3::new(
                velocity.x,
                -STEP_SPEED,
                velocity.z,
            ));
            if counter % 6 == 0 {
                if let Some(stepping) = self.stepping.get() {
                    stepping.tick_stepping(world.clone(), self.move_to_target_pos_goal.target_pos);
                } else {
                    self.tick_stepping(world.clone(), self.move_to_target_pos_goal.target_pos);
                }
            }
        }

        if counter > DESTROY_TICKS {
            // Replace with air, no drops.
            world.set_block_state(&tweak_pos, BlockStateId::AIR, BlockFlags::NOTIFY_ALL);
            // TODO: spawn the poof particles
            if let Some(stepping) = self.stepping.get() {
                stepping.on_destroy_block(world, tweak_pos);
            } else {
                self.on_destroy_block(world, tweak_pos);
            }
        }

        self.counter += 1;
    }

    fn should_run_every_tick(&self) -> bool {
        self.move_to_target_pos_goal.should_run_every_tick()
    }

    fn controls(&self) -> Controls {
        self.move_to_target_pos_goal.controls()
    }
}

impl<S: Stepping + Send + Sync, M: MoveToTargetPos + Send + Sync> MoveToTargetPos
    for StepAndDestroyBlockGoal<S, M>
{
    fn is_target_pos(&self, world: &World, block_pos: BlockPos) -> bool {
        world.get_block(&block_pos).id == self.target_block.id
            && world.get_block_state(&block_pos.up()).is_air()
            && world.get_block_state(&block_pos.up_height(2)).is_air()
    }
}

impl<S: Stepping + Send + Sync, M: MoveToTargetPos + Send + Sync> Stepping
    for StepAndDestroyBlockGoal<S, M>
{
    fn tick_stepping(&self, _world: Arc<World>, _block_pos: BlockPos) {}

    fn on_destroy_block(&self, _world: Arc<World>, _block_pos: BlockPos) {}
}
