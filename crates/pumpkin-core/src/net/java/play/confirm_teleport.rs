#[allow(clippy::wildcard_imports)]
use super::*;

impl JavaClient {
    pub fn handle_confirm_teleport(&self, player: &Player, confirm_teleport: &SConfirmTeleport) {
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
                    // We should set the position now to what we requested in the teleport packet.
                    // This may fix issues when the client sends the position while being teleported.
                    player.get_entity().set_pos(*position);
                    *awaiting_teleport = None;
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
            TeleportResult::Success | TeleportResult::Ignored => {}
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
