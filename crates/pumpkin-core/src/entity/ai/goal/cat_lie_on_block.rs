use pumpkin_data::tag::{self, Taggable};
use pumpkin_util::math::position::BlockPos;

use super::move_to_target_pos::{MoveToTargetPos, MoveToTargetPosGoal};
use super::{Controls, Goal, ParentHandle};
use crate::entity::mob::Mob;
use crate::entity::passive::cat::as_cat;
use crate::entity::passive::tamable::TamableAnimal;
use crate::world::World;

/// Vanilla `CatLieOnBlockGoal`: a tame cat walks to a bed nearby and lies down on it.
pub struct CatLieOnBlockGoal {
    move_to_block: MoveToTargetPosGoal<Self>,
}

impl CatLieOnBlockGoal {
    #[must_use]
    pub fn new(speed: f64, search_range: i32) -> Box<Self> {
        let mut this = Box::new(Self {
            move_to_block: MoveToTargetPosGoal::new(ParentHandle::none(), speed, search_range, 6),
        });
        this.move_to_block.lowest_y = -2;
        // SAFETY: the boxed goal never moves, so the handle stays valid for the goal's lifetime.
        this.move_to_block.move_to_target_pos = unsafe { ParentHandle::new(&this) };
        this
    }
}

impl Goal for CatLieOnBlockGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        as_cat(mob).is_some_and(|cat| cat.is_tame() && !cat.is_ordered_to_sit() && !cat.is_lying())
            && self.move_to_block.can_start(mob)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.move_to_block.should_continue(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.move_to_block.start(mob);
        if let Some(cat) = as_cat(mob) {
            cat.set_in_sitting_pose(false);
        }
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.move_to_block.stop(mob);
        if let Some(cat) = as_cat(mob) {
            cat.set_lying(false);
        }
    }

    fn tick(&mut self, mob: &dyn Mob) {
        self.move_to_block.tick(mob);
        let Some(cat) = as_cat(mob) else {
            return;
        };
        cat.set_in_sitting_pose(false);
        if !self.move_to_block.is_reached_target() {
            cat.set_lying(false);
        } else if !cat.is_lying() {
            cat.set_lying(true);
        }
    }

    fn should_run_every_tick(&self) -> bool {
        self.move_to_block.should_run_every_tick()
    }

    fn controls(&self) -> Controls {
        self.move_to_block.controls()
    }
}

impl MoveToTargetPos for CatLieOnBlockGoal {
    fn is_target_pos(&self, world: &World, block_pos: BlockPos) -> bool {
        world.get_block_state(&block_pos.up()).is_air()
            && world
                .get_block(&block_pos)
                .has_tag(&tag::Block::MINECRAFT_CATS_CAN_LIE_ON)
    }

    fn next_start_tick(&self, _mob: &dyn Mob) -> i32 {
        40
    }
}
