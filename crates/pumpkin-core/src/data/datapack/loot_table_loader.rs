use pumpkin_util::loot_table::DynamicLootTable;
use pumpkin_util::loot_table::parser::{
    LootResolver, parse_loot_table_with, parse_loot_table_with_predicates,
};
use serde_json::Value;
use std::{collections::HashMap, fs, path::Path, sync::Arc};

use super::context_provider_loader::ContextProviderRegistry;
use super::{LOOT_TABLE_REGISTRY, RegistryLoadErrors, refuse_element};
use crate::world::loot::nbt_to_json;

#[must_use]
pub fn parse_loot_table(json: &str) -> Option<DynamicLootTable> {
    parse_loot_table_with_predicates(json, &|name| {
        pumpkin_data::loot_table::get_loot_predicate(name).cloned()
    })
}

/// The loot registries of the packs, which loot tables refer to by id.
pub struct PackLootRegistries<'a> {
    pub predicates: &'a HashMap<String, Value>,
    pub item_modifiers: &'a HashMap<String, Value>,
    pub int_providers: &'a ContextProviderRegistry,
    pub float_providers: &'a ContextProviderRegistry,
    pub slot_sources: &'a HashMap<String, Value>,
}

impl LootResolver for PackLootRegistries<'_> {
    fn predicate(&self, id: &str) -> Option<Value> {
        self.predicates
            .get(id)
            .cloned()
            .or_else(|| pumpkin_data::loot_table::get_loot_predicate(id).cloned())
    }

    fn item_modifier(&self, id: &str) -> Option<Value> {
        self.item_modifiers.get(id).cloned()
    }

    fn int_provider(&self, id: &str) -> Option<Value> {
        self.int_providers.get(id).map(nbt_to_json)
    }

    fn float_provider(&self, id: &str) -> Option<Value> {
        self.float_providers.get(id).map(nbt_to_json)
    }

    fn slot_source(&self, id: &str) -> Option<Value> {
        self.slot_sources.get(id).cloned()
    }
}

/// Reads the loot table files of a directory recursively into `sources`, by id; a later pack's
/// file replaces an earlier one's.
pub fn load_loot_tables_from_dir<S: std::hash::BuildHasher>(
    namespace: &str,
    dir: &Path,
    sources: &mut HashMap<String, String, S>,
    errors: &mut RegistryLoadErrors,
) -> usize {
    if !dir.is_dir() {
        return 0;
    }
    let mut loaded = 0;
    load_loot_tables_recursive(namespace, dir, dir, sources, &mut loaded, errors);
    loaded
}

fn load_loot_tables_recursive<S: std::hash::BuildHasher>(
    namespace: &str,
    base_dir: &Path,
    current_dir: &Path,
    sources: &mut HashMap<String, String, S>,
    loaded: &mut usize,
    errors: &mut RegistryLoadErrors,
) {
    let Ok(entries) = fs::read_dir(current_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            load_loot_tables_recursive(namespace, base_dir, &path, sources, loaded, errors);
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
            let loot_table_id = format!("{namespace}:{stem_path}");

            match fs::read_to_string(&path) {
                Ok(content) => {
                    sources.insert(loot_table_id, content);
                    *loaded += 1;
                }
                Err(error) => errors.fail(LOOT_TABLE_REGISTRY, loot_table_id, error),
            }
        }
    }
}

/// Parses the loot tables of every pack, now that the registries they refer to are complete.
pub fn parse_loot_tables<S: std::hash::BuildHasher>(
    sources: &HashMap<String, String, S>,
    registries: &PackLootRegistries<'_>,
    errors: &mut RegistryLoadErrors,
) -> HashMap<String, Arc<DynamicLootTable>> {
    let mut tables = HashMap::new();
    for (id, content) in sources {
        if let Some(table) = parse_loot_table_with(content, registries) {
            errors.loaded(LOOT_TABLE_REGISTRY, id);
            tables.insert(id.clone(), Arc::new(table));
        } else {
            refuse_element(errors, LOOT_TABLE_REGISTRY, id.clone(), content);
        }
    }
    tables
}

#[cfg(test)]
mod tests {
    use super::*;
    use pumpkin_util::loot_table::{
        DynamicLootCondition, DynamicLootEntryKind, LootBonusFormula, LootFunctionKind,
        LootNumberProvider,
    };

    #[test]
    fn parse_simple_loot_table() {
        let json = r#"{
            "type": "minecraft:chest",
            "pools": [
                {
                    "rolls": 2,
                    "entries": [
                        {
                            "type": "minecraft:item",
                            "name": "minecraft:diamond",
                            "weight": 5,
                            "functions": [
                                {
                                    "function": "minecraft:set_count",
                                    "count": {"min": 1, "max": 3}
                                }
                            ]
                        },
                        {
                            "type": "minecraft:empty",
                            "weight": 10
                        }
                    ]
                }
            ]
        }"#;

        let table = parse_loot_table(json).expect("valid loot table");
        assert_eq!(table.pools.len(), 1);
        let pool = &table.pools[0];
        assert_eq!(pool.rolls, LootNumberProvider::Constant(2.0));
        assert_eq!(pool.entries.len(), 2);
        assert_eq!(pool.entries[1].weight, 10);
        assert!(matches!(pool.entries[1].kind, DynamicLootEntryKind::Empty));
        assert!(
            matches!(&pool.entries[0].kind, DynamicLootEntryKind::Item(name) if name == "minecraft:diamond")
        );
        assert_eq!(pool.entries[0].weight, 5);
        assert!(matches!(
            pool.entries[0].functions[0].kind,
            LootFunctionKind::SetCount {
                count: LootNumberProvider::Uniform { min: 1.0, max: 3.0 },
                add: false
            }
        ));
    }

    #[test]
    fn parse_block_loot_with_silk_touch_and_fortune() {
        let json = r#"{
            "type": "minecraft:block",
            "pools": [
                {
                    "rolls": 1,
                    "entries": [
                        {
                            "type": "minecraft:alternatives",
                            "children": [
                                {
                                    "type": "minecraft:item",
                                    "name": "minecraft:coal_ore",
                                    "conditions": [
                                        {
                                            "condition": "minecraft:match_tool",
                                            "predicate": {
                                                "predicates": {
                                                    "minecraft:enchantments": [
                                                        {"enchantments": "minecraft:silk_touch"}
                                                    ]
                                                }
                                            }
                                        }
                                    ]
                                },
                                {
                                    "type": "minecraft:item",
                                    "name": "minecraft:coal",
                                    "functions": [
                                        {
                                            "function": "minecraft:apply_bonus",
                                            "formula": "minecraft:ore_drops"
                                        }
                                    ],
                                    "conditions": [
                                        {
                                            "condition": "minecraft:survives_explosion"
                                        }
                                    ]
                                }
                            ]
                        }
                    ]
                }
            ]
        }"#;

        let table = parse_loot_table(json).expect("valid block loot table");
        assert_eq!(table.pools.len(), 1);
        let pool = &table.pools[0];
        let DynamicLootEntryKind::Alternatives(children) = &pool.entries[0].kind else {
            panic!("expected alternatives")
        };
        assert_eq!(children.len(), 2);
        assert!(
            matches!(&children[0].kind, DynamicLootEntryKind::Item(name) if name == "minecraft:coal_ore")
        );
        assert!(matches!(
            children[0].condition,
            DynamicLootCondition::MatchTool(_)
        ));
        assert!(
            matches!(&children[1].kind, DynamicLootEntryKind::Item(name) if name == "minecraft:coal")
        );
        assert!(matches!(
            children[1].functions[0].kind,
            LootFunctionKind::ApplyBonus(LootBonusFormula::OreDrops)
        ));
        assert_eq!(
            children[1].condition,
            DynamicLootCondition::SurvivesExplosion
        );
    }

    #[test]
    fn directory_loading() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("data/custom/loot_table");
        let sub = root.join("blocks");
        fs::create_dir_all(&sub).expect("create dir");

        let table_json = r#"{
            "pools": [
                {
                    "rolls": 1,
                    "condition": "custom:disabled",
                    "entries": [
                        {
                            "type": "minecraft:item",
                            "name": "minecraft:dirt"
                        }
                    ]
                }
            ]
        }"#;
        fs::write(sub.join("dirt.json"), table_json).expect("write json");

        let mut sources = HashMap::new();
        let mut errors = RegistryLoadErrors::default();
        let count = load_loot_tables_from_dir("custom", &root, &mut sources, &mut errors);
        assert_eq!(count, 1);
        // The predicate comes from the registries of every pack, loaded or not before the table.
        let predicates = HashMap::from([(
            "custom:disabled".to_owned(),
            serde_json::json!({"type": "minecraft:random_chance", "chance": 0}),
        )]);
        let empty_values = HashMap::new();
        let empty_providers = HashMap::new();
        let registry = parse_loot_tables(
            &sources,
            &PackLootRegistries {
                predicates: &predicates,
                item_modifiers: &empty_values,
                int_providers: &empty_providers,
                float_providers: &empty_providers,
                slot_sources: &empty_values,
            },
            &mut errors,
        );
        assert!(matches!(
            registry["custom:blocks/dirt"].pools[0].condition,
            DynamicLootCondition::RandomChance { chance: 0.0 }
        ));
    }

    #[test]
    fn unsupported_tables_are_left_out_and_broken_ones_fail() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("data/custom/loot_table");
        fs::create_dir_all(&root).expect("create dir");
        // A JSON object with an entry type Pumpkin can't read.
        fs::write(
            root.join("unsupported.json"),
            r#"{"pools":[{"rolls":1,"entries":[{"type":"minecraft:future_entry"}]}]}"#,
        )
        .expect("write json");
        fs::write(root.join("broken.json"), r#"{"pools": ["#).expect("write json");

        let mut sources = HashMap::new();
        let mut errors = RegistryLoadErrors::default();
        load_loot_tables_from_dir("custom", &root, &mut sources, &mut errors);
        let empty_values = HashMap::new();
        let empty_providers = HashMap::new();
        let registry = parse_loot_tables(
            &sources,
            &PackLootRegistries {
                predicates: &empty_values,
                item_modifiers: &empty_values,
                int_providers: &empty_providers,
                float_providers: &empty_providers,
                slot_sources: &empty_values,
            },
            &mut errors,
        );
        assert!(registry.is_empty());
        assert!(
            errors
                .get(LOOT_TABLE_REGISTRY, "custom:unsupported")
                .is_none()
        );
        assert!(errors.get(LOOT_TABLE_REGISTRY, "custom:broken").is_some());
    }
}
