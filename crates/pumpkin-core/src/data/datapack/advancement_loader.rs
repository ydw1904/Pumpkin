use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

use super::{ADVANCEMENT_REGISTRY, RegistryLoadErrors};

/// An advancement definition from a datapack, built by
/// [`crate::data::advancement_tree::ServerAdvancements`].
#[derive(Clone, Debug)]
pub struct DynamicAdvancementDefinition {
    pub id: String,
    pub json: Value,
}

/// Loads all advancement JSON files from a directory recursively into `registry`.
pub fn load_advancements_from_dir<S: std::hash::BuildHasher>(
    namespace: &str,
    dir: &Path,
    registry: &mut HashMap<String, DynamicAdvancementDefinition, S>,
    errors: &mut RegistryLoadErrors,
) -> usize {
    if !dir.is_dir() {
        return 0;
    }
    let before = registry.len();
    load_advancements_recursive(namespace, dir, dir, registry, errors);
    registry.len() - before
}

fn load_advancements_recursive<S: std::hash::BuildHasher>(
    namespace: &str,
    base_dir: &Path,
    current_dir: &Path,
    registry: &mut HashMap<String, DynamicAdvancementDefinition, S>,
    errors: &mut RegistryLoadErrors,
) {
    let Ok(entries) = fs::read_dir(current_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            load_advancements_recursive(namespace, base_dir, &path, registry, errors);
        } else if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
            && let Ok(rel_path) = path.strip_prefix(base_dir)
        {
            let mut stem_path = rel_path.to_string_lossy().to_string();
            if let Some(stem) = stem_path.strip_suffix(".json") {
                stem_path = stem.to_string();
            }
            let stem_path = stem_path.replace('\\', "/");
            let adv_id = format!("{namespace}:{stem_path}");

            match fs::read_to_string(&path)
                .map_err(|error| error.to_string())
                .and_then(|content| {
                    serde_json::from_str::<Value>(&content).map_err(|error| error.to_string())
                }) {
                Ok(json) => {
                    errors.loaded(ADVANCEMENT_REGISTRY, &adv_id);
                    registry.insert(
                        adv_id.clone(),
                        DynamicAdvancementDefinition { id: adv_id, json },
                    );
                }
                Err(error) => errors.fail(ADVANCEMENT_REGISTRY, adv_id, error),
            }
        }
    }
}
