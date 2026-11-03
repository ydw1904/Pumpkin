#[allow(clippy::wildcard_imports)]
use super::*;
use pumpkin_protocol::java::server::play::SContainerSlotStateChanged;

impl JavaClient {
    pub fn handle_container_slot_state_changed(
        &self,
        player: &Player,
        packet: &SContainerSlotStateChanged,
    ) {
        // ServerGamePacketListenerImpl.handleContainerSlotStateChanged: only crafters use it.
        if player.gamemode.load() == GameMode::Spectator {
            return;
        }
        let screen_handler = player
            .current_screen_handler
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let mut screen_handler = screen_handler
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if i32::from(screen_handler.sync_id()) != packet.container_id.0 {
            return;
        }
        if let Some(crafter) = screen_handler
            .as_any_mut()
            .downcast_mut::<pumpkin_inventory::crafter_screen_handler::CrafterScreenHandler>(
        ) && let Ok(slot) = usize::try_from(packet.slot_id.0)
        {
            crafter.set_slot_state(slot, packet.new_state);
        }
    }
}
