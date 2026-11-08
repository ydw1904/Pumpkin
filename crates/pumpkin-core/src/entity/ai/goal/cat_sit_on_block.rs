use pumpkin_data::Block;
use pumpkin_data::block_properties::{BedPart, FurnaceLikeProperties, WhiteBedLikeProperties};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_util::math::position::BlockPos;

use super::move_to_target_pos::{MoveToTargetPos, MoveToTargetPosGoal};
use super::{Controls, Goal, ParentHandle};
use crate::block::entities::chest::ChestBlockEntity;
use crate::entity::mob::Mob;
use crate::entity::passive::cat::as_cat;
use crate::entity::passive::tamable::TamableAnimal;
use crate::world::World;

/// Vanilla `CatSitOnBlockGoal`: a tame cat sits on a closed chest, a lit furnace or the foot of
/// a bed nearby.
pub struct CatSitOnBlockGoal {
    move_to_block: MoveToTargetPosGoal<Self>,
}

impl CatSitOnBlockGoal {
    #[must_use]
    pub fn new(speed: f64) -> Box<Self> {
        let mut this = Box::new(Self {
            move_to_block: MoveToTargetPosGoal::with_default(ParentHandle::none(), speed, 8),
        });
        // SAFETY: the boxed goal never moves, so the handle stays valid for the goal's lifetime.
        this.move_to_block.move_to_target_pos = unsafe { ParentHandle::new(&this) };
        this
    }
}

impl Goal for CatSitOnBlockGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        as_cat(mob).is_some_and(|cat| cat.is_tame() && !cat.is_ordered_to_sit())
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
            cat.set_in_sitting_pose(false);
        }
    }

    fn tick(&mut self, mob: &dyn Mob) {
        self.move_to_block.tick(mob);
        if let Some(cat) = as_cat(mob) {
            cat.set_in_sitting_pose(self.move_to_block.is_reached_target());
        }
    }

    fn should_run_every_tick(&self) -> bool {
        self.move_to_block.should_run_every_tick()
    }

    fn controls(&self) -> Controls {
        self.move_to_block.controls()
    }
}

impl MoveToTargetPos for CatSitOnBlockGoal {
    fn is_target_pos(&self, world: &World, block_pos: BlockPos) -> bool {
        if !world.get_block_state(&block_pos.up()).is_air() {
            return false;
        }
        let (block, state_id) = world.get_block_and_state_id(&block_pos);
        if !block.has_tag(&tag::Block::MINECRAFT_CATS_CAN_SIT_ON) {
            return false;
        }
        if block == &Block::CHEST {
            // Vanilla `ChestBlockEntity.getOpenCount`.
            return world.get_block_entity(&block_pos).is_none_or(|entity| {
                entity
                    .as_any()
                    .downcast_ref::<ChestBlockEntity>()
                    .is_none_or(|chest| chest.get_viewer_count() < 1)
            });
        }
        if block == &Block::FURNACE {
            return FurnaceLikeProperties::from_state_id(state_id).lit;
        }
        if block.has_tag(&tag::Block::MINECRAFT_BEDS) {
            return WhiteBedLikeProperties::from_state_id(state_id).part != BedPart::Head;
        }
        true
    }
}
