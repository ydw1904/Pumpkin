use std::sync::Arc;

use super::pick_up_block::enderman_block_reach;
use super::{Goal, GoalReach, to_goal_ticks};
use crate::entity::mob::Mob;
use crate::entity::mob::enderman::EndermanEntity;
use crate::world::game_event::GameEventContext;
use pumpkin_data::block_properties::is_air;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::{Block, BlockState};
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::world::BlockFlags;

pub struct PlaceBlockGoal {
    enderman: Arc<EndermanEntity>,
}

impl PlaceBlockGoal {
    pub const fn new(enderman: Arc<EndermanEntity>) -> Self {
        Self { enderman }
    }
}

impl Goal for PlaceBlockGoal {
    fn tick_reach(&self, mob: &dyn Mob, _running: bool) -> Option<GoalReach> {
        Some(enderman_block_reach(mob))
    }

    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        if self.enderman.get_carried_block().is_none() {
            return false;
        }

        let entity = &mob.get_mob_entity().living_entity.entity;
        let world = entity.world.load();
        if !world.level_info.load().game_rules.mob_griefing {
            return false;
        }

        if mob.get_random().random_range(0..to_goal_ticks(2000)) != 0 {
            return false;
        }

        true
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let Some(block_state_id) = self.enderman.get_carried_block() else {
            return;
        };

        let entity = &mob.get_mob_entity().living_entity.entity;
        let pos = entity.pos.load();

        let (bx, by, bz) = {
            let rng = mob.get_random();
            (
                (pos.x - 1.0 + rng.random::<f64>() * 2.0).floor() as i32,
                (pos.y + rng.random::<f64>() * 2.0).floor() as i32,
                (pos.z - 1.0 + rng.random::<f64>() * 2.0).floor() as i32,
            )
        };

        let world = entity.world.load();
        let target_pos = BlockPos::new(bx, by, bz);

        let state_id = world.get_block_state_id(&target_pos);
        if !is_air(state_id) {
            return;
        }

        let below_pos = BlockPos::new(bx, by - 1, bz);
        let (below_block, below_state) = world.get_block_and_state(&below_pos);
        if !below_state.is_solid() || !below_state.is_full_cube() || below_block == &Block::BEDROCK
        {
            return;
        }

        // Never place into a block another entity occupies.
        let world_clone = entity.world.load_full();
        if world_clone
            .get_all_at_box(&BoundingBox::from_block(&target_pos))
            .iter()
            .any(|other| other.get_entity().entity_id != entity.entity_id)
        {
            return;
        }

        let carried_block = Block::from_state_id(block_state_id);
        let carried_state = BlockState::from_id(block_state_id);
        if !world_clone.block_registry.can_place_at(
            None,
            Some(&world_clone),
            world_clone.as_ref(),
            None,
            carried_block,
            carried_state,
            &target_pos,
            None,
            None,
        ) {
            return;
        }

        world_clone.set_block_state(&target_pos, block_state_id, BlockFlags::NOTIFY_ALL);
        world_clone.emit_game_event_with_context(
            GameEvent::BlockPlace.name(),
            target_pos.to_centered_f64(),
            &GameEventContext {
                source_entity: Some(self.enderman.as_ref()),
            },
        );
        self.enderman.set_carried_block(None);
    }
}
