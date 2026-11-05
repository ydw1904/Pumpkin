#[allow(clippy::wildcard_imports)]
use super::*;

impl JavaClient {
    pub fn handle_seen_advancement(&self, player: &Arc<Player>, packet: &SSeenAdvancement) {
        if let SSeenAdvancement::OpenTab(tab) = packet {
            let mut advancements = player
                .advancements
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let advancement = advancements.tree.get(tab).map(|entry| entry.advancement);
            if advancement.is_some() {
                advancements.set_selected_tab(advancement);
            }
        }
    }
}
