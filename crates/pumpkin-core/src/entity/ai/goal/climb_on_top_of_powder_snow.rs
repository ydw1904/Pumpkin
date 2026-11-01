use std::sync::atomic::Ordering::Relaxed;

use pumpkin_data::Block;
use pumpkin_data::tag::{self, Taggable};

use super::{Controls, Goal};
use crate::entity::mob::Mob;

/// Vanilla `ClimbOnTopOfPowderSnowGoal`: mobs that can walk on powder snow jump out of it.
pub struct ClimbOnTopOfPowderSnowGoal;

impl Goal for ClimbOnTopOfPowderSnowGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let entity = mob.get_entity();
        if !(entity.was_in_powder_snow.load(Relaxed) || entity.is_in_powder_snow())
            || !entity
                .entity_type
                .has_tag(&tag::EntityType::MINECRAFT_POWDER_SNOW_WALKABLE_MOBS)
        {
            return false;
        }

        let pos = entity.block_pos.load();
        let above = entity.world.load().get_block_state(&pos.up());
        above.id.to_block() == &Block::POWDER_SNOW
            || above.get_block_collision_shapes_at(&pos).next().is_none()
    }

    fn tick(&mut self, mob: &dyn Mob) {
        mob.get_mob_entity()
            .jump_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .jump();
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        Controls::JUMP
    }
}
