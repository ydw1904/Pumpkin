use std::{path::PathBuf, sync::Arc};

use pumpkin_config::world::LevelConfig;
use pumpkin_data::dimension::Dimension;

use crate::level::Level;

#[must_use]
pub fn into_level(
    dimension: Dimension,
    level_config: &LevelConfig,
    base_directory: PathBuf,
    seed: i64,
) -> Arc<Level> {
    let level = Level::from_root_folder(level_config, base_directory, seed, dimension);
    // Vanilla's ServerLevel constructor starts the ring search. Bare levels (tests)
    // only start it when a lookup first needs it.
    if let Some(cache) = level.world_gen().global_structure_cache() {
        cache.ensure_strongholds_generated();
    }
    level
}
