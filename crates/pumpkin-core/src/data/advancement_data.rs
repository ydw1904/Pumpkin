use crate::data::advancement_tree::ServerAdvancements;
use crate::entity::player::Player;
use crate::entity::player::advancement::{AdvancementDataError, PlayerAdvancement};
use arc_swap::ArcSwap;
use pumpkin_util::identifier::Identifier;
use std::fs::create_dir_all;
use std::path::PathBuf;
use std::slice;
use std::sync::Arc;
use tracing::error;
use uuid::Uuid;

/// Manages player advancements, including data creation and saving.
pub struct AdvancementManager {
    pub advancement_path: PathBuf,
    pub save_enabled: bool,
    advancements: ArcSwap<ServerAdvancements>,
}

impl AdvancementManager {
    /// Creates a new instance of `AdvancementManager` using the player data path.
    pub fn new(player_data_path: impl Into<PathBuf>, save_enabled: bool) -> Self {
        let path = player_data_path.into().join("advancements");
        if !path.exists()
            && let Err(e) = create_dir_all(&path)
        {
            error!(
                "Failed to create player data directory at {}: {e}",
                path.display()
            );
        }
        Self {
            advancement_path: path,
            save_enabled,
            advancements: ArcSwap::from_pointee(ServerAdvancements::default()),
        }
    }

    /// The advancements of the enabled datapacks (vanilla `MinecraftServer.getAdvancements`).
    #[must_use]
    pub fn advancements(&self) -> Arc<ServerAdvancements> {
        self.advancements.load_full()
    }

    /// Replaces the advancements after a datapack reload. Players keep the ones they have
    /// until [`PlayerAdvancement::reload`].
    pub fn set_advancements(&self, advancements: ServerAdvancements) {
        self.advancements.store(Arc::new(advancements));
    }

    /// The ids of every advancement.
    #[must_use]
    pub fn get_advancements(&self) -> Vec<Identifier> {
        self.advancements()
            .entries()
            .iter()
            .map(|entry| entry.advancement.id.clone())
            .collect()
    }

    /// Creates and returns a new instance of `PlayerAdvancement` with the configured path.
    #[inline]
    #[must_use]
    pub fn new_player_advancement(self: Arc<Self>, owner: Uuid) -> PlayerAdvancement {
        PlayerAdvancement::new(self, owner)
    }

    /// Saves the advancements of all provided players.
    pub async fn save_all_players(
        &self,
        players: &[Arc<Player>],
    ) -> Result<(), AdvancementDataError> {
        if !self.save_enabled {
            return Ok(());
        }
        let mut to_write = Vec::with_capacity(players.len());
        for player in players {
            let guard = player
                .advancements
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let json = serde_json::to_string_pretty(&*guard).map_err(AdvancementDataError::Json)?;
            to_write.push((guard.path.clone(), json));
        }

        if let Err(e) = tokio::fs::create_dir_all(&self.advancement_path).await {
            error!("Failed to create player advancement directory : {e}");
            return Err(AdvancementDataError::Io(e));
        }
        for (path, json) in to_write {
            tokio::fs::write(&path, json)
                .await
                .map_err(AdvancementDataError::Io)?;
        }
        Ok(())
    }

    /// Saves the advancements of a specific player.
    pub async fn save_player(&self, player: &Arc<Player>) -> Result<(), AdvancementDataError> {
        self.save_all_players(slice::from_ref(player)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn advancement_manager_new() {
        let path = PathBuf::from("test_data");
        let manager = AdvancementManager::new(path, true);
        assert_eq!(
            manager.advancement_path,
            PathBuf::from("test_data/advancements")
        );
    }

    #[test]
    fn get_advancement_path() {
        let path = PathBuf::from("world/playerdata");
        let manager = AdvancementManager::new(path, true);
        let advancement_path = manager.advancement_path;
        assert!(advancement_path.ends_with("advancements"));
    }
}
