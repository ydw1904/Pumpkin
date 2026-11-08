use crate::entity::vehicle::boat::BoatEntity;

#[allow(clippy::wildcard_imports)]
use super::*;

impl JavaClient {
    pub fn handle_player_command(
        &self,
        player: &Arc<Player>,
        command: &SPlayerCommand,
        server: &Arc<Server>,
    ) {
        if command.entity_id != player.entity_id().into() {
            return;
        }
        if !player.has_client_loaded() {
            return;
        }
        player.update_last_action_time();

        let entity = &player.get_entity();
        match command.action {
            Action::StartSprinting => player.apply_sprint_input(server, true),
            Action::StopSprinting => player.apply_sprint_input(server, false),
            Action::LeaveBed => player.wake_up(),

            Action::StartHorseJump => player.start_riding_jump(server, command.jump_boost.0),
            // Releasing the jump key does nothing on any rideable mob.
            Action::StopHorseJump => {}
            // Vanilla opens any vehicle that is a `HasCustomInventoryScreen`.
            Action::OpenVehicleInventory => {
                let vehicle = player.get_entity().get_vehicle();
                if let Some(boat) = vehicle
                    .as_ref()
                    .and_then(|vehicle| vehicle.cast_any().downcast_ref::<BoatEntity>())
                {
                    boat.open_inventory(player);
                } else {
                    player.open_vehicle_inventory();
                }
            }
            Action::StartFlyingElytra => {
                if !player.try_to_start_fall_flying(server) {
                    entity.stop_fall_flying();
                }
            }
            // <= 1.21.5
            Action::StartSneaking | Action::StopSneaking => {
                self.handle_player_input(
                    player,
                    &SPlayerInput {
                        input: SPlayerInput::SNEAK,
                    },
                    server,
                );
            }
        }
    }
}
