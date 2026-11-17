use crate::{
    entity::{NBTStorage, player::Player},
    server::Server,
};
use crossbeam::atomic::AtomicCell;
use pumpkin_inventory::screen_handler::ScreenHandler;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_world::data::player_data::{PlayerDataError, PlayerDataStorage};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use tracing::{debug, error};
use uuid::Uuid;

/// Helper for managing player data in the server context.
///
/// This struct provides server-wide access to the `PlayerDataStorage` and
/// convenience methods for player handling.
pub struct ServerPlayerData {
    storage: Arc<PlayerDataStorage>,
    /// Vanilla's `players/stats`, holding a `<uuid>.json` per player.
    stats_path: PathBuf,
    save_interval: Duration,
    last_save: AtomicCell<Instant>,
    order: Arc<SaveOrder>,
}

/// Keeps a snapshot taken earlier (a periodic save still being written off the tick) from
/// replacing one taken later (the save on disconnect).
#[derive(Default)]
struct SaveOrder {
    next_generation: AtomicU64,
    /// The generation last written for each player. Holding it also serializes the writes.
    written: Mutex<HashMap<Uuid, u64>>,
}

struct PlayerSnapshot {
    uuid: Uuid,
    generation: u64,
    nbt: NbtCompound,
    stats: serde_json::Value,
}

impl SaveOrder {
    fn snapshot(&self, player: &Player) -> PlayerSnapshot {
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed);
        let mut nbt = NbtCompound::new();
        player.write_nbt(&mut nbt);
        PlayerSnapshot {
            uuid: player.gameprofile.id,
            generation,
            nbt,
            stats: stats_json(player),
        }
    }

    fn write(
        &self,
        storage: &PlayerDataStorage,
        stats_path: &Path,
        snapshot: PlayerSnapshot,
    ) -> Result<(), PlayerDataError> {
        let mut written = self
            .written
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if written
            .get(&snapshot.uuid)
            .is_some_and(|&last| last > snapshot.generation)
        {
            debug!(
                "Skipping an outdated player data save for {}",
                snapshot.uuid
            );
            return Ok(());
        }
        storage.save_player_data(&snapshot.uuid, snapshot.nbt)?;
        if storage.is_save_enabled() {
            save_stats(stats_path, snapshot.uuid, &snapshot.stats);
        }
        written.insert(snapshot.uuid, snapshot.generation);
        Ok(())
    }
}

impl ServerPlayerData {
    /// Creates a new `ServerPlayerData` with specified configuration.
    pub fn new(
        data_path: impl Into<PathBuf>,
        stats_path: impl Into<PathBuf>,
        save_interval: Duration,
        enabled: bool,
    ) -> Self {
        Self {
            storage: Arc::new(PlayerDataStorage::new(data_path, enabled)),
            stats_path: stats_path.into(),
            save_interval,
            last_save: AtomicCell::new(Instant::now()),
            order: Arc::default(),
        }
    }

    /// Handles a player leaving the server.
    ///
    /// This function saves player data when they disconnect.
    ///
    /// # Arguments
    ///
    /// * `player` - The player who left.
    ///
    /// # Returns
    ///
    /// A Result indicating success or the error that occurred.
    pub fn handle_player_leave(&self, player: &Arc<Player>) -> Result<(), PlayerDataError> {
        player
            .player_screen_handler
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .on_closed(player.as_ref());
        player.on_handled_screen_closed();

        let snapshot = self.order.snapshot(player);
        self.order.write(&self.storage, &self.stats_path, snapshot)
    }

    /// Reads a player's `players/stats/<uuid>.json`, if they have one.
    #[must_use]
    pub fn load_stats(&self, uuid: &uuid::Uuid) -> Option<serde_json::Value> {
        if !self.storage.is_save_enabled() {
            return None;
        }
        let path = self.stats_path.join(format!("{uuid}.json"));
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
            Err(e) => {
                error!("Couldn't read statistics file {}: {e}", path.display());
                return None;
            }
        };
        serde_json::from_str(&text)
            .inspect_err(|e| error!("Couldn't parse statistics file {}: {e}", path.display()))
            .ok()
    }

    /// Performs periodic maintenance tasks.
    ///
    /// This function is called synchronously on the server tick loop to check
    /// if it is time to save player data.
    pub fn tick(&self, server: &Server) {
        let now = Instant::now();

        // Only save players periodically based on save_interval
        let last_save = self.last_save.load();
        let should_save = now.duration_since(last_save) >= self.save_interval;

        if should_save && self.storage.is_save_enabled() {
            self.last_save.store(now);
            // Snapshot all online players periodically across all worlds
            let mut snapshots = Vec::new();
            for world in server.worlds.load().iter() {
                for player in world.players.load().iter() {
                    snapshots.push(self.order.snapshot(player));
                }
            }

            if snapshots.is_empty() {
                return;
            }

            let storage = self.storage.clone();
            let stats_path = self.stats_path.clone();
            let order = self.order.clone();
            // The tick runs on a simulation thread with no Tokio context, so use the
            // server's runtime handle instead of `tokio::task::spawn_blocking`.
            server.runtime.spawn_blocking(move || {
                for snapshot in snapshots {
                    let uuid = snapshot.uuid;
                    if let Err(e) = order.write(&storage, &stats_path, snapshot) {
                        error!("Failed to save player data for {uuid}: {e}");
                    }
                }
                debug!("Periodic player data save completed");
            });
        }
    }

    /// Saves all players' data immediately.
    ///
    /// This function immediately saves all online players' data to disk.
    /// Useful for server shutdown or backup operations.
    pub fn save_all_players(&self, server: &Server) -> Result<(), PlayerDataError> {
        let mut total_players = 0;

        // Save players from all worlds
        for world in server.worlds.load().iter() {
            for player in world.players.load().iter() {
                self.extract_data_and_save_player(player)?;
                total_players += 1;
            }
        }

        debug!("Saved data for {total_players} online players");
        Ok(())
    }

    /// Loads player data and applies it to a player.
    ///
    /// This function loads a player's data and applies it to their Player instance.
    /// For new players, it creates default data without errors.
    ///
    /// # Arguments
    ///
    /// * `player` - The player to load data for and apply to.
    ///
    /// # Returns
    ///
    /// A Result indicating success or the error that occurred.
    pub fn load_data(&self, uuid: &uuid::Uuid) -> Result<Option<NbtCompound>, PlayerDataError> {
        match self.storage.load_player_data(uuid) {
            Ok((true, data)) => return Ok(Some(data)),
            Ok((false, _)) => {}
            Err(e) => {
                error!("Error loading player data for {uuid}: {e}");
                self.back_up_corrupted(uuid);
            }
        }

        // Vanilla `PlayerDataStorage.load` falls back to `.dat_old` when `.dat` is missing or
        // broken, and starts with default data only when that fails too.
        match self.storage.load_old_player_data(uuid) {
            Ok((true, data)) => Ok(Some(data)),
            Ok((false, _)) => Ok(None),
            Err(e) => {
                error!("Error loading old player data for {uuid}: {e}");
                Ok(None)
            }
        }
    }

    /// Vanilla `PlayerDataStorage.backup`: keeps a copy of an unreadable `.dat`, since the next
    /// save replaces it.
    fn back_up_corrupted(&self, uuid: &Uuid) {
        const FORMAT: &[time::format_description::FormatItem<'static>] =
            time::macros::format_description!("[year]-[month]-[day]_[hour]-[minute]-[second]");
        let path = self.storage.get_player_data_path(uuid);
        if !path.is_file() {
            return;
        }
        let now =
            time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
        let date = now.format(FORMAT).unwrap_or_default();
        let backup = path.with_file_name(format!("{uuid}_corrupted_{date}.dat"));
        if let Err(e) = std::fs::copy(&path, &backup) {
            error!("Failed to copy the player data file for {uuid}: {e}");
        }
    }

    /// Extracts and saves data from a player.
    ///
    /// This function extracts NBT data from a player and saves it to disk.
    ///
    /// # Arguments
    ///
    /// * `player` - The player to extract and save data for.
    ///
    /// # Returns
    ///
    /// A Result indicating success or the error that occurred.
    pub fn extract_data_and_save_player(&self, player: &Player) -> Result<(), PlayerDataError> {
        if !self.storage.is_save_enabled() {
            return Ok(());
        }

        let snapshot = self.order.snapshot(player);
        self.order.write(&self.storage, &self.stats_path, snapshot)
    }
}

fn stats_json(player: &Player) -> serde_json::Value {
    player
        .stats
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .to_json(pumpkin_world::world_info::MAXIMUM_SUPPORTED_WORLD_DATA_VERSION)
}

/// Vanilla `ServerStatsCounter.save`.
fn save_stats(stats_path: &std::path::Path, uuid: uuid::Uuid, stats: &serde_json::Value) {
    let path = stats_path.join(format!("{uuid}.json"));
    let result = std::fs::create_dir_all(stats_path).and_then(|()| {
        let json = serde_json::to_string_pretty(stats).map_err(std::io::Error::other)?;
        std::fs::write(&path, json)
    });
    if let Err(e) = result {
        error!("Couldn't save stats to {}: {e}", path.display());
    }
}

#[cfg(test)]
mod test {
    use crate::data::player_server::{PlayerSnapshot, SaveOrder, ServerPlayerData};
    use pumpkin_nbt::compound::NbtCompound;
    use pumpkin_world::data::player_data::PlayerDataStorage;
    use std::time::Duration;
    use std::time::Instant;
    use tempfile::tempdir;
    use uuid::Uuid;

    #[tokio::test]
    async fn player_data_storage_new() {
        // Create a temporary directory for testing
        let temp_dir = tempdir().unwrap();
        let path = temp_dir.path().to_path_buf();

        let storage = PlayerDataStorage::new(path.clone(), true);

        assert_eq!(storage.get_data_path().as_path(), path.as_path());
        // Note: save_enabled might be configured differently in your actual code
    }

    #[tokio::test]
    async fn player_data_storage_get_player_data_path() {
        let temp_dir = tempdir().unwrap();
        let path = temp_dir.path().to_path_buf();

        let storage = PlayerDataStorage::new(path.clone(), true);

        let uuid = Uuid::new_v4();
        let expected_path = path.join(format!("{uuid}.dat"));

        assert_eq!(storage.get_player_data_path(&uuid), expected_path);
    }

    #[tokio::test]
    async fn player_data_storage_save_and_load() {
        let temp_dir = tempdir().unwrap();
        let path = temp_dir.path().to_path_buf();

        let storage = PlayerDataStorage::new(path, true); // Ensure saving is enabled for this test

        let uuid = Uuid::new_v4();

        // Create test data
        let mut nbt = NbtCompound::new();
        nbt.put_string("TestKey", "TestValue".to_string());
        nbt.put_int("TestInt", 42);

        // Save the data
        storage.save_player_data(&uuid, nbt).unwrap();

        // Load the data
        let (load_success, loaded_nbt) = storage.load_player_data(&uuid).unwrap();

        assert!(load_success);
        assert_eq!(loaded_nbt.get_string("TestKey").unwrap(), "TestValue");
        assert_eq!(loaded_nbt.get_int("TestInt").unwrap(), 42);
    }

    #[tokio::test]
    async fn player_data_storage_load_nonexistent() {
        let temp_dir = tempdir().unwrap();
        let path = temp_dir.path().to_path_buf();

        let storage = PlayerDataStorage::new(path, true); // Ensure saving is enabled for this test

        let uuid = Uuid::new_v4();

        // Try to load non-existent data
        let (load_success, empty_nbt) = storage.load_player_data(&uuid).unwrap();

        assert!(!load_success);
        assert_eq!(empty_nbt.child_tags.len(), 0);
    }

    #[tokio::test]
    async fn player_data_storage_disabled() {
        let temp_dir = tempdir().unwrap();
        let path = temp_dir.path().to_path_buf();

        let storage = PlayerDataStorage::new(path, false);

        let uuid = Uuid::new_v4();
        let mut nbt = NbtCompound::new();
        nbt.put_string("TestKey", "TestValue".to_string());

        // Save should succeed but do nothing
        let save_result = storage.save_player_data(&uuid, nbt);
        assert!(save_result.is_ok());

        // Load should return empty data
        let (load_success, empty_nbt) = storage.load_player_data(&uuid).unwrap();
        assert!(!load_success);
        assert_eq!(empty_nbt.child_tags.len(), 0);
    }

    #[tokio::test]
    async fn server_player_data_new() {
        let temp_dir = tempdir().unwrap();
        let path = temp_dir.path().to_path_buf();
        let save_interval = Duration::from_mins(5);

        let player_data =
            ServerPlayerData::new(path.clone(), path.join("stats"), save_interval, true);

        assert_eq!(player_data.save_interval, save_interval);
        assert!(
            Instant::now().duration_since(player_data.last_save.load()) < Duration::from_secs(1)
        );
    }

    #[tokio::test]
    async fn player_data_file_structure() {
        let temp_dir = tempdir().unwrap();
        let path = temp_dir.path().to_path_buf();

        let uuid = Uuid::new_v4();
        let storage = PlayerDataStorage::new(path, true);

        // Create and save player data
        let mut nbt = NbtCompound::new();
        nbt.put_string("name", "TestPlayer".to_string());
        nbt.put_int("level", 42);
        storage.save_player_data(&uuid, nbt).unwrap();

        // Verify the file exists
        let player_data_path = storage.get_player_data_path(&uuid);
        assert!(player_data_path.exists());

        // Load it again and verify content
        let (success, loaded_data) = storage.load_player_data(&uuid).unwrap();
        assert!(success);
        assert_eq!(loaded_data.get_string("name").unwrap(), "TestPlayer");
        assert_eq!(loaded_data.get_int("level").unwrap(), 42);
    }

    #[test]
    fn older_snapshot_does_not_replace_newer_save() {
        let temp_dir = tempdir().unwrap();
        let storage = PlayerDataStorage::new(temp_dir.path(), true);
        let stats = temp_dir.path().join("stats");
        let order = SaveOrder::default();
        let uuid = Uuid::new_v4();
        let snapshot = |generation, level| {
            let mut nbt = NbtCompound::new();
            nbt.put_int("level", level);
            PlayerSnapshot {
                uuid,
                generation,
                nbt,
                stats: serde_json::Value::Null,
            }
        };

        // A periodic snapshot taken first but written after the disconnect save.
        order.write(&storage, &stats, snapshot(2, 2)).unwrap();
        order.write(&storage, &stats, snapshot(1, 1)).unwrap();

        let (_, loaded) = storage.load_player_data(&uuid).unwrap();
        assert_eq!(loaded.get_int("level").unwrap(), 2);
    }

    #[test]
    fn corrupt_player_data_is_backed_up_and_old_copy_loaded() {
        let temp_dir = tempdir().unwrap();
        let path = temp_dir.path();
        let player_data =
            ServerPlayerData::new(path, path.join("stats"), Duration::from_mins(5), true);
        let uuid = Uuid::new_v4();
        let mut nbt = NbtCompound::new();
        nbt.put_int("level", 1);
        player_data.storage.save_player_data(&uuid, nbt).unwrap();
        let mut nbt = NbtCompound::new();
        nbt.put_int("level", 2);
        player_data.storage.save_player_data(&uuid, nbt).unwrap();
        std::fs::write(path.join(format!("{uuid}.dat")), b"not gzip").unwrap();

        let loaded = player_data.load_data(&uuid).unwrap().unwrap();

        assert_eq!(loaded.get_int("level").unwrap(), 1);
        let backups = std::fs::read_dir(path)
            .unwrap()
            .filter(|entry| {
                entry
                    .as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(&format!("{uuid}_corrupted_"))
            })
            .count();
        assert_eq!(backups, 1);
    }
}
