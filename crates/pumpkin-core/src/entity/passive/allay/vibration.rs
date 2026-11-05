use pumpkin_data::{game_event::GameEvent, tag};
use pumpkin_util::math::{position::BlockPos, vector3::Vector3};

use super::{AllayEntity, ai};
use crate::entity::EntityBase;
use crate::entity::vibration::{VibrationInfo, is_occluded, is_valid_vibration};
use crate::world::{
    World,
    game_event::{GameEventContext, GameEventListener},
};

const VIBRATION_EVENT_LISTENER_RANGE: i32 = 16;

pub(super) struct JukeboxListener<'a>(pub(super) &'a AllayEntity);

impl GameEventListener for JukeboxListener<'_> {
    fn listener_position(&self) -> Vector3<f64> {
        self.0.get_eye_pos()
    }

    fn listener_radius(&self) -> i32 {
        GameEvent::JukeboxPlay.notification_radius()
    }

    fn handle_game_event(
        &self,
        _world: &World,
        event: GameEvent,
        _context: &GameEventContext<'_>,
        source_position: Vector3<f64>,
    ) -> bool {
        let playing = match event {
            GameEvent::JukeboxPlay => true,
            GameEvent::JukeboxStopPlay => false,
            _ => return false,
        };
        self.0
            .set_jukebox_playing(BlockPos::floored_v(source_position), playing);
        true
    }
}

pub(super) struct VibrationListener<'a>(pub(super) &'a AllayEntity);

impl GameEventListener for VibrationListener<'_> {
    fn listener_position(&self) -> Vector3<f64> {
        self.0.get_eye_pos()
    }

    fn listener_radius(&self) -> i32 {
        VIBRATION_EVENT_LISTENER_RANGE
    }

    fn handle_game_event(
        &self,
        world: &World,
        event: GameEvent,
        context: &GameEventContext<'_>,
        source_position: Vector3<f64>,
    ) -> bool {
        let allay = self.0;
        if allay
            .vibration
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .has_current()
            || !is_valid_vibration(&tag::GameEvent::MINECRAFT_ALLAY_CAN_LISTEN, event, context)
        {
            return false;
        }
        let destination = allay.get_eye_pos();
        if !allay.can_receive_vibration(world, BlockPos::floored_v(source_position))
            || is_occluded(world, source_position, destination)
        {
            return false;
        }
        let time = world.get_world_age();
        let distance = (source_position - destination).length() as f32;
        let mut data = allay
            .vibration
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        data.add_candidate(
            VibrationInfo::new(event, distance, source_position, context.source_entity),
            time,
        );
        true
    }
}

impl AllayEntity {
    fn can_receive_vibration(&self, world: &World, pos: BlockPos) -> bool {
        if self.mob_entity.is_no_ai() {
            return false;
        }
        let entity = self.get_entity();
        self.mob_entity.with_brain(self, |brain| {
            brain
                .get(crate::entity::ai::brain::memory::types::LIKED_NOTEBLOCK_POSITION)
                .is_none_or(|liked| {
                    liked.is_close_enough(
                        &world.dimension,
                        entity.block_pos.load(),
                        ai::MAX_NOTEBLOCK_DISTANCE,
                    ) && liked.pos == pos
                })
        })
    }

    pub(super) fn tick_vibration(&self) {
        let entity = self.get_entity();
        let world = entity.world.load();
        let received = self
            .vibration
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .tick(
                &world,
                entity.entity_id,
                entity.get_eye_height() as f32,
                self.get_eye_pos(),
            );
        if let Some(event) = received.filter(|event| event.event == GameEvent::NoteBlockPlay) {
            ai::hear_noteblock(self, BlockPos::floored_v(event.pos));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::{
        Entity,
        ai::{brain::memory::types, test_support::world},
    };
    use pumpkin_data::entity::EntityType;

    #[tokio::test]
    async fn wool_occludes_notes_but_a_solid_stone_block_does_not()
    -> Result<(), Box<dyn std::error::Error>> {
        let folder = tempfile::tempdir()?;
        let world = world(&folder);
        world.level.loaded_chunks.insert(
            pumpkin_util::math::vector2::Vector2::new(0, 0),
            pumpkin_world::chunk::ChunkData::empty_sync(0, 0),
        );
        let allay = AllayEntity::new(Entity::new(
            world.clone(),
            Vector3::new(6.5, 64.0, 2.5),
            &EntityType::ALLAY,
        ));
        let blocker = BlockPos::new(4, 64, 2);
        let origin = BlockPos::new(2, 64, 2).to_centered_f64();
        world.set_block_state(
            &blocker,
            pumpkin_data::Block::STONE.default_state.id,
            pumpkin_world::world::BlockFlags::empty(),
        );
        VibrationListener(&allay).handle_game_event(
            &world,
            GameEvent::NoteBlockPlay,
            &GameEventContext {
                source_entity: None,
            },
            origin,
        );
        assert!(
            allay
                .vibration
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take_candidate()
                .is_some()
        );
        world.set_block_state(
            &blocker,
            pumpkin_data::Block::WHITE_WOOL.default_state.id,
            pumpkin_world::world::BlockFlags::empty(),
        );
        VibrationListener(&allay).handle_game_event(
            &world,
            GameEvent::NoteBlockPlay,
            &GameEventContext {
                source_entity: None,
            },
            origin,
        );
        assert!(
            allay
                .vibration
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take_candidate()
                .is_none()
        );
        world.level.shutdown().await;
        Ok(())
    }

    #[tokio::test]
    async fn note_delivery_waits_for_travel_and_survives_reload()
    -> Result<(), Box<dyn std::error::Error>> {
        let folder = tempfile::tempdir()?;
        let world = world(&folder);
        let allay = AllayEntity::new(Entity::new(
            world.clone(),
            Vector3::new(3.5, 64.0, 0.5),
            &EntityType::ALLAY,
        ));
        let origin = BlockPos::new(0, 64, 0);
        VibrationListener(&allay).handle_game_event(
            &world,
            GameEvent::NoteBlockPlay,
            &GameEventContext {
                source_entity: None,
            },
            origin.to_centered_f64(),
        );
        allay.tick_vibration();
        assert!(
            !allay
                .vibration
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .has_current()
        );
        world
            .level_time
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .world_age = 1;
        allay.tick_vibration();
        let nbt = allay
            .vibration
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .write_nbt();
        assert_eq!(nbt.get_int("event_delay"), Some(2));
        assert!(!allay.mob_entity.with_brain(allay.as_ref(), |brain| {
            brain.has_memory_value(types::LIKED_NOTEBLOCK_POSITION.id())
        }));
        *allay
            .vibration
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            crate::entity::vibration::VibrationData::read_nbt(&nbt);
        allay.tick_vibration();
        assert!(!allay.mob_entity.with_brain(allay.as_ref(), |brain| {
            brain.has_memory_value(types::LIKED_NOTEBLOCK_POSITION.id())
        }));
        allay.tick_vibration();
        assert_eq!(
            allay.mob_entity.with_brain(allay.as_ref(), |brain| brain
                .get(types::LIKED_NOTEBLOCK_POSITION)
                .map(|liked| liked.pos)),
            Some(origin)
        );
        world.level.shutdown().await;
        Ok(())
    }
}
