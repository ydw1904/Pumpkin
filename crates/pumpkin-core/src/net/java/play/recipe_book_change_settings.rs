#[allow(clippy::wildcard_imports)]
use super::*;

impl JavaClient {
    pub fn handle_recipe_book_change_settings(
        &self,
        server: &Arc<Server>,
        player: &Arc<Player>,
        packet: &SRecipeBookChangeSettings,
    ) {
        let mut event = crate::plugin::api::events::player::player_recipe_book_settings_change::PlayerRecipeBookSettingsChangeEvent::new(
            player.clone(),
            format!("{:?}", packet.book_type),
            packet.is_open,
            packet.is_filtering,
        );
        server.plugin_manager.fire_blocking(server, &mut event);
        if event.cancelled {
            return;
        }
        if let Ok(book_type) = usize::try_from(packet.book_type.0) {
            player
                .recipe_book
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .set_book_setting(book_type, packet.is_open, packet.is_filtering);
        }
    }
}
