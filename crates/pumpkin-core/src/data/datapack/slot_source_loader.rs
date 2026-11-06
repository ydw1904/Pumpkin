use std::collections::HashMap;
use std::fs;
use std::path::Path;

use serde_json::Value;

use super::{RegistryLoadErrors, SLOT_SOURCE_REGISTRY};

pub fn load_slot_sources_from_dir<S: std::hash::BuildHasher>(
    namespace: &str,
    dir: &Path,
    slot_sources: &mut HashMap<String, Value, S>,
    errors: &mut RegistryLoadErrors,
) -> usize {
    let mut loaded = 0;
    load_slot_sources_recursive(namespace, dir, dir, slot_sources, &mut loaded, errors);
    loaded
}

fn load_slot_sources_recursive<S: std::hash::BuildHasher>(
    namespace: &str,
    base_dir: &Path,
    current_dir: &Path,
    slot_sources: &mut HashMap<String, Value, S>,
    loaded: &mut usize,
    errors: &mut RegistryLoadErrors,
) {
    let Ok(entries) = fs::read_dir(current_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            load_slot_sources_recursive(namespace, base_dir, &path, slot_sources, loaded, errors);
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
            let slot_source_id = format!("{namespace}:{stem_path}");

            match fs::read_to_string(&path)
                .map_err(|error| error.to_string())
                .and_then(|content| {
                    serde_json::from_str::<Value>(&content).map_err(|error| error.to_string())
                }) {
                Ok(val) => {
                    errors.loaded(SLOT_SOURCE_REGISTRY, &slot_source_id);
                    slot_sources.insert(slot_source_id, val);
                    *loaded += 1;
                }
                Err(error) => errors.fail(SLOT_SOURCE_REGISTRY, slot_source_id, error),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_load_slot_sources() {
        let temp = tempfile::tempdir().expect("tempdir");
        let slot_dir = temp.path().join("slot_source");
        fs::create_dir_all(&slot_dir).expect("create dir");
        fs::write(
            slot_dir.join("test_slot.json"),
            r#"{"source": "minecraft:armor"}"#,
        )
        .expect("write");

        let mut map = HashMap::new();
        let loaded = load_slot_sources_from_dir(
            "custom",
            &slot_dir,
            &mut map,
            &mut RegistryLoadErrors::default(),
        );
        assert_eq!(loaded, 1);
        assert!(map.contains_key("custom:test_slot"));
    }
}
