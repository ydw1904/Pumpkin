#[allow(clippy::wildcard_imports)]
use super::*;

impl JavaClient {
    pub fn handle_confirm_teleport(
        &self,
        player: &Arc<Player>,
        server: &Arc<Server>,
        confirm_teleport: &SConfirmTeleport,
    ) {
        enum TeleportResult {
            Success,
            Ignored,
            NotTeleporting,
        }

        let result = {
            let mut awaiting_teleport = player
                .awaiting_teleport
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some((id, position)) = awaiting_teleport.as_ref() {
                if id == &confirm_teleport.teleport_id {
                    let pos = confirm_teleport.position;
                    if pos.x.is_nan()
                        || pos.y.is_nan()
                        || pos.z.is_nan()
                        || !confirm_teleport.yaw.is_finite()
                        || !confirm_teleport.pitch.is_finite()
                    {
                        drop(awaiting_teleport);
                        self.try_kick(&TextComponent::translate(
                            translation::java::MULTIPLAYER_DISCONNECT_INVALID_PLAYER_MOVEMENT,
                            [],
                        ));
                        return;
                    }
                    // We should set the position now to what we requested in the teleport packet.
                    // This may fix issues when the client sends the position while being teleported.
                    player.get_entity().set_pos(*position);
                    *awaiting_teleport = None;
                    // Vanilla `hasChangedDimension`.
                    player.changing_dimension.store(false, Ordering::Relaxed);
                    TeleportResult::Success
                } else {
                    // A newer teleport replaced this one, vanilla ignores the stale id
                    TeleportResult::Ignored
                }
            } else if confirm_teleport.teleport_id.0
                == player.teleport_id_count.load(Ordering::Relaxed)
            {
                TeleportResult::NotTeleporting
            } else {
                TeleportResult::Ignored
            }
        };

        match result {
            // Vanilla then handles the position the client accepted like a move packet
            // that is not on the ground and has no horizontal collision.
            TeleportResult::Success => self.handle_position_rotation(
                player,
                server,
                &SPlayerPositionRotation {
                    position: confirm_teleport.position,
                    yaw: confirm_teleport.yaw,
                    pitch: confirm_teleport.pitch,
                    collision: 0,
                },
            ),
            TeleportResult::Ignored => {}
            // Vanilla `handleAcceptTeleportPacket`: the current id confirmed a second time
            TeleportResult::NotTeleporting => {
                self.try_kick(&TextComponent::translate(
                    translation::java::MULTIPLAYER_DISCONNECT_INVALID_PLAYER_MOVEMENT,
                    [],
                ));
            }
        }
    }
}
