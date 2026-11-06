use std::fs;
use std::path::Path;
use std::sync::Arc;
use tracing::error;

/// The structure templates, template pools and processor lists the packs define. They replace
/// the registered ones in [`StagedWorldgen::apply`], once the whole load has succeeded.
#[derive(Default)]
pub struct StagedWorldgen {
    templates: Vec<(String, Arc<[u8]>)>,
    template_pools: Vec<(String, String)>,
    processor_lists: Vec<(String, String)>,
}

impl StagedWorldgen {
    /// Replaces the registered structure templates, template pools and processor lists.
    pub fn apply(self) {
        clear_dynamic_worldgen_data();
        for (id, bytes) in self.templates {
            pumpkin_world::generation::structure::template::global_cache()
                .register_template(&id, bytes);
        }
        for (id, json) in self.template_pools {
            if let Err(e) =
                pumpkin_world::generation::structure::structures::jigsaw::register_dynamic_template_pool_json(
                    &id, &json,
                )
            {
                error!("Failed to parse template pool '{id}': {e}");
            }
        }
        for (id, json) in self.processor_lists {
            if let Err(e) =
                pumpkin_world::generation::structure::template::processor::register_dynamic_processor_list(
                    &id, &json,
                )
            {
                error!("Failed to parse processor list '{id}': {e}");
            }
        }
    }
}

/// Recursively scans `<namespace_dir>/structure/` for `.nbt` structure templates.
pub fn load_structures_from_dir(
    namespace: &str,
    structure_dir: &Path,
    staged: &mut StagedWorldgen,
) -> usize {
    let before = staged.templates.len();
    visit_dir(
        structure_dir,
        namespace,
        "",
        "nbt",
        &mut |resource_id, path| match fs::read(path) {
            Ok(bytes) => staged
                .templates
                .push((resource_id, Arc::from(bytes.into_boxed_slice()))),
            Err(e) => {
                error!("Failed to read structure template '{resource_id}' from {path:?}: {e}");
            }
        },
    );
    staged.templates.len() - before
}

/// Recursively scans `<namespace_dir>/worldgen/template_pool/` for `.json` template pool definitions.
pub fn load_template_pools_from_dir(
    namespace: &str,
    template_pool_dir: &Path,
    staged: &mut StagedWorldgen,
) -> usize {
    let before = staged.template_pools.len();
    visit_json_dir(template_pool_dir, namespace, &mut staged.template_pools);
    staged.template_pools.len() - before
}

/// Recursively scans `<namespace_dir>/worldgen/processor_list/` for `.json` processor lists.
pub fn load_processor_lists_from_dir(
    namespace: &str,
    processor_list_dir: &Path,
    staged: &mut StagedWorldgen,
) -> usize {
    let before = staged.processor_lists.len();
    visit_json_dir(processor_list_dir, namespace, &mut staged.processor_lists);
    staged.processor_lists.len() - before
}

fn visit_json_dir(dir: &Path, namespace: &str, files: &mut Vec<(String, String)>) {
    visit_dir(
        dir,
        namespace,
        "",
        "json",
        &mut |resource_id, path| match fs::read_to_string(path) {
            Ok(json) => files.push((resource_id, json)),
            Err(e) => error!("Failed to read '{resource_id}' from {path:?}: {e}"),
        },
    );
}

/// Calls `visit` with the resource id and path of every file with the extension under `dir`.
fn visit_dir(
    dir: &Path,
    namespace: &str,
    prefix: &str,
    extension: &str,
    visit: &mut dyn FnMut(String, &Path),
) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_name) = entry.file_name().into_string() else {
            continue;
        };

        if path.is_dir() {
            let next_prefix = if prefix.is_empty() {
                file_name
            } else {
                format!("{prefix}/{file_name}")
            };
            visit_dir(&path, namespace, &next_prefix, extension, visit);
            continue;
        }

        if !path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case(extension))
        {
            continue;
        }

        let Some(stem) = path.file_stem() else {
            continue;
        };
        let stem = stem.to_string_lossy();
        let name = if prefix.is_empty() {
            stem.to_string()
        } else {
            format!("{prefix}/{stem}")
        };
        visit(format!("{namespace}:{name}"), &path);
    }
}

/// Clears all dynamically registered worldgen structures, template pools, and processor lists.
pub fn clear_dynamic_worldgen_data() {
    pumpkin_world::generation::structure::template::global_cache().clear_dynamic();
    pumpkin_world::generation::structure::structures::jigsaw::clear_dynamic_template_pools();
    pumpkin_world::generation::structure::template::processor::clear_dynamic_processors();
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn load_structure_template_and_template_pool() {
        let dir = tempdir().unwrap();
        let data_dir = dir.path().join("data").join("test_ns");
        let structure_dir = data_dir.join("structure");
        let template_pool_dir = data_dir.join("worldgen").join("template_pool");
        fs::create_dir_all(&structure_dir).unwrap();
        fs::create_dir_all(&template_pool_dir).unwrap();

        // Write a test template pool JSON
        let pool_json = r#"{
            "fallback": "minecraft:empty",
            "elements": [
                {
                    "weight": 10,
                    "element": {
                        "element_type": "minecraft:empty_pool_element"
                    }
                }
            ]
        }"#;
        fs::write(template_pool_dir.join("test_pool.json"), pool_json).unwrap();

        let mut staged = StagedWorldgen::default();
        let pools_loaded = load_template_pools_from_dir("test_ns", &template_pool_dir, &mut staged);
        assert_eq!(pools_loaded, 1);
        staged.apply();

        let discovered =
            pumpkin_world::generation::structure::structures::jigsaw::TemplatePool::discover(
                "test_ns:test_pool",
            );
        assert!(discovered.is_some());
        let pool = discovered.unwrap();
        assert_eq!(pool.elements.len(), 1);
        assert_eq!(pool.elements[0].weight, 10);

        clear_dynamic_worldgen_data();
    }
}
