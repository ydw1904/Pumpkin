use std::sync::PoisonError;

use pumpkin_data::{game_event::GameEvent, tag};
use pumpkin_util::math::{position::BlockPos, vector3::Vector3};

use super::WardenEntity;
use crate::entity::EntityBase;
use crate::entity::player::advancement::trigger::AdvancementTrigger;
use crate::entity::vibration::{VibrationInfo, is_occluded, is_valid_vibration};
use crate::world::{
    World,
    game_event::{GameEventContext, GameEventListener},
};

/// Vanilla `Warden.VibrationUser.GAME_EVENT_LISTENER_RANGE`.
const GAME_EVENT_LISTENER_RANGE: i32 = 16;

pub(super) struct WardenVibrationListener<'a>(pub(super) &'a WardenEntity);

impl GameEventListener for WardenVibrationListener<'_> {
    fn listener_position(&self) -> Vector3<f64> {
        self.0.get_entity().get_eye_pos()
    }

    fn listener_radius(&self) -> i32 {
        GAME_EVENT_LISTENER_RANGE
    }

    fn handle_game_event(
        &self,
        world: &World,
        event: GameEvent,
        context: &GameEventContext<'_>,
        source_position: Vector3<f64>,
    ) -> bool {
        let warden = self.0;
        let listenable = &tag::GameEvent::MINECRAFT_WARDEN_CAN_LISTEN;
        if warden
            .vibration
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .has_current()
        {
            return false;
        }
        if !is_valid_vibration(listenable, event, context) {
            trigger_avoid_vibration(listenable, event, context);
            return false;
        }
        let destination = self.listener_position();
        if !warden.can_receive_vibration(
            world,
            BlockPos::floored_v(source_position),
            context.source_entity,
        ) || is_occluded(world, source_position, destination)
        {
            return false;
        }
        let distance = (source_position - destination).length() as f32;
        warden
            .vibration
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .add_candidate(
                VibrationInfo::new(event, distance, source_position, context.source_entity),
                world.get_world_age(),
            );
        true
    }
}

/// Vanilla `VibrationSystem.User.isValidVibration` for users that `canTriggerAvoidVibration`:
/// a player sneaking past one of them earns `minecraft:avoid_vibration`.
fn trigger_avoid_vibration(
    listenable: &tag::Tag,
    event: GameEvent,
    context: &GameEventContext<'_>,
) {
    let Some(source) = context.source_entity else {
        return;
    };
    if listenable.0.contains(&event.name())
        && !source.is_spectator()
        && source.get_entity().is_sneaking()
        && tag::GameEvent::MINECRAFT_IGNORE_VIBRATIONS_SNEAKING
            .0
            .contains(&event.name())
        && let Some(player) = source.get_player()
    {
        player.trigger_advancement(AdvancementTrigger::AvoidVibration);
    }
}
