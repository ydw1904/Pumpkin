#[allow(clippy::wildcard_imports)]
use super::*;

impl JavaClient {
    pub fn handle_recipe_book_seen_recipe(
        &self,
        server: &Arc<Server>,
        player: &Arc<Player>,
        packet: &SRecipeBookSeenRecipe,
    ) {
        let mut event = crate::plugin::api::events::player::player_recipe_discover::PlayerRecipeDiscoverEvent::new(
            player.clone(),
            format!("display_{}", packet.recipe_display_id.0),
        );
        server.plugin_manager.fire_blocking(server, &mut event);
        if event.cancelled {
            return;
        }
        if let Some(recipe_id) = player.recipe_from_display(packet.recipe_display_id.0) {
            player
                .recipe_book
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove_highlight(&recipe_id);
        }
    }
}
