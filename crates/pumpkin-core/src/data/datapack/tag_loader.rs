//! Vanilla `TagLoader`: the tag files of every pack, stacked per tag and resolved per registry.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs;
use std::path::Path;

use pumpkin_data::datapack_tags::LoadedTag;
use pumpkin_data::entity::EntityType;
use pumpkin_data::fluid::Fluid;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::poi_type::PoiType;
use pumpkin_data::potion::Potion;
use pumpkin_data::tag::{
    RegistryKey, get_builtin_entry_id, get_builtin_tag_entries, get_latest_map,
};
use pumpkin_data::{Block, item::Item};
use pumpkin_util::identifier::Identifier;
use serde_json::Value;
use tracing::error;

use super::dynamic_registry_manager::DynamicRegistryManager;

/// The pack id vanilla reports for the entries of the built-in tags.
const VANILLA_PACK: &str = "vanilla";

/// Vanilla `TagLoader.EntryWithSource`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagEntry {
    /// The full id of the element or tag.
    pub id: String,
    pub tag: bool,
    pub required: bool,
    pub source: String,
}

impl TagEntry {
    /// Parses a vanilla `TagEntry` string as the built-in tags store it: `#` for a tag, a
    /// trailing `?` for an optional entry.
    fn builtin(entry: &str) -> Self {
        let (entry, required) = entry
            .strip_suffix('?')
            .map_or((entry, true), |entry| (entry, false));
        let (id, tag) = entry
            .strip_prefix('#')
            .map_or((entry, false), |id| (id, true));
        Self {
            id: full_id(id),
            tag,
            required,
            source: VANILLA_PACK.to_string(),
        }
    }
}

impl fmt::Display for TagEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.tag {
            f.write_str("#")?;
        }
        f.write_str(&self.id)?;
        if !self.required {
            f.write_str("?")?;
        }
        write!(f, " (from {})", self.source)
    }
}

/// The entries the packs give one tag, in pack order.
#[derive(Clone, Debug, Default)]
pub struct TagBuilder {
    pub entries: Vec<TagEntry>,
    /// A pack set `replace`, so the entries of the packs below it, the built-in tag included,
    /// are gone.
    pub replace: bool,
}

/// Tag builders by registry path (`item`, `worldgen/biome`, ...) and tag id.
pub type TagBuilders = HashMap<String, HashMap<String, TagBuilder>>;

fn full_id(id: &str) -> String {
    if id.contains(':') {
        id.to_string()
    } else {
        format!("minecraft:{id}")
    }
}

/// The value a tag lists for an element: its id, without the `minecraft` namespace like the
/// built-in tags.
fn value_name(id: &str) -> &str {
    id.strip_prefix("minecraft:").unwrap_or(id)
}

/// Loads every tag file under a namespace's `tags` directory into `builders`, returning how many
/// files were read. A file that fails to parse is logged and skipped (vanilla `TagLoader.load`).
pub fn load_tags_from_dir(
    namespace: &str,
    tags_dir: &Path,
    pack: &str,
    builders: &mut TagBuilders,
) -> usize {
    let mut loaded = 0;
    let Ok(entries) = fs::read_dir(tags_dir) else {
        return 0;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name == "worldgen" {
            // Worldgen registries are nested, e.g. tags/worldgen/biome.
            let Ok(worldgen) = fs::read_dir(&path) else {
                continue;
            };
            for sub in worldgen.flatten() {
                let sub_path = sub.path();
                if sub_path.is_dir() {
                    let registry = format!("worldgen/{}", sub.file_name().to_string_lossy());
                    let tags = builders.entry(registry).or_default();
                    load_registry_tags(namespace, &sub_path, &sub_path, pack, tags, &mut loaded);
                }
            }
        } else {
            let tags = builders.entry(name).or_default();
            load_registry_tags(namespace, &path, &path, pack, tags, &mut loaded);
        }
    }
    loaded
}

fn load_registry_tags(
    namespace: &str,
    base_dir: &Path,
    current_dir: &Path,
    pack: &str,
    tags: &mut HashMap<String, TagBuilder>,
    loaded: &mut usize,
) {
    let Ok(entries) = fs::read_dir(current_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            load_registry_tags(namespace, base_dir, &path, pack, tags, loaded);
            continue;
        }
        if !path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
        {
            continue;
        }
        let Ok(rel_path) = path.strip_prefix(base_dir) else {
            continue;
        };
        let stem = rel_path.with_extension("");
        let tag_id = format!("{namespace}:{}", stem.to_string_lossy().replace('\\', "/"));
        let builder = tags.entry(tag_id.clone()).or_default();
        let parsed = fs::read_to_string(&path)
            .map_err(|err| err.to_string())
            .and_then(|content| parse_tag_file(&content, pack));
        match parsed {
            Ok((file_entries, replace)) => {
                if replace {
                    builder.entries.clear();
                    builder.replace = true;
                }
                builder.entries.extend(file_entries);
                *loaded += 1;
            }
            Err(err) => error!(
                "Couldn't read tag list {tag_id} from {} in data pack {pack}: {err}",
                path.display()
            ),
        }
    }
}

/// Vanilla `TagFile.CODEC`: `values` is required, `replace` defaults to false.
fn parse_tag_file(content: &str, pack: &str) -> Result<(Vec<TagEntry>, bool), String> {
    let json: Value = serde_json::from_str(content).map_err(|err| err.to_string())?;
    let replace = match json.get("replace") {
        None => false,
        Some(value) => value
            .as_bool()
            .ok_or_else(|| format!("Not a boolean: {value}"))?,
    };
    let values = json
        .get("values")
        .ok_or("No key values in MapLike")?
        .as_array()
        .ok_or("values is not a list")?;
    let entries = values
        .iter()
        .map(|value| parse_tag_entry(value, pack))
        .collect::<Result<_, _>>()?;
    Ok((entries, replace))
}

/// Vanilla `TagEntry.CODEC`: `"id"`, `"#tag"` or `{"id": ..., "required": ...}`.
fn parse_tag_entry(value: &Value, pack: &str) -> Result<TagEntry, String> {
    let (id, required) = match value {
        Value::String(id) => (id.as_str(), true),
        Value::Object(map) => {
            let id = map
                .get("id")
                .ok_or("No key id in MapLike")?
                .as_str()
                .ok_or("id is not a string")?;
            let required = match map.get("required") {
                None => true,
                Some(required) => required
                    .as_bool()
                    .ok_or_else(|| format!("Not a boolean: {required}"))?,
            };
            (id, required)
        }
        other => return Err(format!("Not a tag entry: {other}")),
    };
    let (id, tag) = id.strip_prefix('#').map_or((id, false), |id| (id, true));
    let identifier = Identifier::parse(id).map_err(|err| err.to_string())?;
    Ok(TagEntry {
        id: identifier.to_string(),
        tag,
        required,
        source: pack.to_string(),
    })
}

/// Looks up a registry element's id by its full id.
pub type ElementLookup<'a> = Box<dyn Fn(&str) -> Option<u16> + 'a>;

/// The element lookup of a registry, or `None` for a registry Pumpkin can't resolve.
///
/// Synchronized data-driven registries use the ids the client gets, datapack entries included.
pub fn element_lookup<'a>(
    registry: &str,
    dynamic: &'a DynamicRegistryManager,
) -> Option<ElementLookup<'a>> {
    fn vanilla(id: &str) -> Option<&str> {
        id.strip_prefix("minecraft:")
    }
    Some(match registry {
        "block" => Box::new(|id| Block::from_registry_key(vanilla(id)?).map(|b| b.id.as_u16())),
        "item" => Box::new(|id| Item::from_registry_key(vanilla(id)?).map(|i| i.id)),
        "fluid" => Box::new(|id| Fluid::from_registry_key(vanilla(id)?).map(|f| f.id)),
        "entity_type" => Box::new(|id| EntityType::from_name(vanilla(id)?).map(|e| e.id)),
        "game_event" => Box::new(|id| GameEvent::from_name(vanilla(id)?).map(|e| e as u16)),
        "potion" => Box::new(|id| Potion::from_name(vanilla(id)?).map(|p| u16::from(p.id))),
        "point_of_interest_type" => {
            Box::new(|id| PoiType::from_name(vanilla(id)?).map(|p| u16::from(p.id())))
        }
        // Villager trade tags are resolved by the trade loader, by trade id. Configured features
        // have no generated ids (their files live in `worldgen/feature`).
        "villager_trade" | "worldgen/configured_feature" => return None,
        _ => {
            let registry_id = format!("minecraft:{registry}");
            if dynamic.contains_registry(&registry_id) {
                Box::new(move |id| {
                    dynamic
                        .get_raw_id(&registry_id, id)
                        .and_then(|raw| u16::try_from(raw).ok())
                })
            } else {
                let key = RegistryKey::from_string(registry)?;
                Box::new(move |id| get_builtin_entry_id(key, id))
            }
        }
    })
}

/// The entries of a built-in tag, or nothing when it has none.
fn builtin_entries(key: RegistryKey, tag: &str) -> Vec<TagEntry> {
    if let Some(entries) = get_builtin_tag_entries(key, tag) {
        return entries
            .iter()
            .map(|entry| TagEntry::builtin(entry))
            .collect();
    }
    get_latest_map(key)
        .get(tag)
        .map_or_else(Vec::new, |builtin| {
            builtin
                .0
                .iter()
                .map(|value| TagEntry::builtin(value))
                .collect()
        })
}

/// Resolves the tags of one registry (vanilla `TagLoader.build`).
///
/// The built-in tags of `key` get the packs' entries stacked on top. A tag missing a required
/// element or tag is logged and left out, like vanilla.
pub fn build_registry_tags<S: std::hash::BuildHasher>(
    key: Option<RegistryKey>,
    builders: &HashMap<String, TagBuilder, S>,
    element: &dyn Fn(&str) -> Option<u16>,
) -> Vec<LoadedTag> {
    let mut contents: HashMap<String, Vec<TagEntry>> = HashMap::new();
    if let Some(key) = key {
        for name in get_latest_map(key).keys() {
            contents.insert((*name).to_string(), builtin_entries(key, name));
        }
    }
    for (name, builder) in builders {
        let entries = contents.entry(name.clone()).or_default();
        if builder.replace {
            entries.clear();
        }
        entries.extend(builder.entries.iter().cloned());
    }

    let mut resolver = Resolver {
        contents: &contents,
        element,
        built: HashMap::new(),
        visiting: HashSet::new(),
    };
    let mut names: Vec<&String> = contents.keys().collect();
    names.sort_unstable();
    for name in names {
        resolver.build(name);
    }
    let mut tags: Vec<LoadedTag> = resolver
        .built
        .into_iter()
        .filter_map(|(name, built)| built.map(|(values, ids)| (name, values, ids)))
        .collect();
    tags.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    tags
}

/// A built tag's values and ids.
type BuiltTag = (Vec<String>, Vec<u16>);

struct Resolver<'a> {
    contents: &'a HashMap<String, Vec<TagEntry>>,
    element: &'a dyn Fn(&str) -> Option<u16>,
    /// Built tags; `None` for one that failed.
    built: HashMap<String, Option<BuiltTag>>,
    visiting: HashSet<String>,
}

impl Resolver<'_> {
    /// Vanilla `TagLoader.tryBuildTag`, building the tags it refers to first like
    /// `DependencySorter`. A tag in a cycle counts as not loaded.
    fn build(&mut self, name: &str) -> Option<&BuiltTag> {
        if !self.built.contains_key(name) {
            let entries = self.contents.get(name)?;
            if !self.visiting.insert(name.to_string()) {
                return None;
            }
            let mut values = Vec::new();
            let mut ids = Vec::new();
            let mut seen = HashSet::new();
            let mut missing = Vec::new();
            for entry in entries {
                if entry.tag {
                    match self.build(&entry.id) {
                        Some((tag_values, tag_ids)) => {
                            for (value, &id) in tag_values.iter().zip(tag_ids) {
                                if seen.insert(id) {
                                    values.push(value.clone());
                                    ids.push(id);
                                }
                            }
                        }
                        None if entry.required => missing.push(entry),
                        None => {}
                    }
                } else {
                    match (self.element)(&entry.id) {
                        Some(id) => {
                            if seen.insert(id) {
                                values.push(value_name(&entry.id).to_string());
                                ids.push(id);
                            }
                        }
                        None if entry.required => missing.push(entry),
                        None => {}
                    }
                }
            }
            self.visiting.remove(name);
            let built = if missing.is_empty() {
                Some((values, ids))
            } else {
                let missing: Vec<String> = missing.iter().map(ToString::to_string).collect();
                error!(
                    "Couldn't load tag {name} as it is missing following references: {}",
                    missing.join(", ")
                );
                None
            };
            self.built.insert(name.to_string(), built);
        }
        self.built.get(name).and_then(Option::as_ref)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn builder(entries: &[&str], replace: bool) -> TagBuilder {
        TagBuilder {
            entries: entries
                .iter()
                .map(|entry| {
                    let mut entry = TagEntry::builtin(entry);
                    entry.source = "file/test".to_string();
                    entry
                })
                .collect(),
            replace,
        }
    }

    fn block_id(id: &str) -> Option<u16> {
        pumpkin_data::Block::from_registry_key(id.strip_prefix("minecraft:")?)
            .map(|b| b.id.as_u16())
    }

    fn resolve<'a>(tags: &'a [LoadedTag], name: &str) -> Option<&'a [String]> {
        tags.iter()
            .find(|tag| tag.0 == name)
            .map(|tag| tag.1.as_slice())
    }

    #[test]
    fn packs_stack_on_builtin_tags_and_nested_tags_follow() {
        let builders = HashMap::from([
            (
                "minecraft:logs".to_string(),
                builder(&["minecraft:stone"], false),
            ),
            ("test:own".to_string(), builder(&["#minecraft:logs"], false)),
        ]);
        let tags = build_registry_tags(Some(RegistryKey::Block), &builders, &block_id);
        let logs = resolve(&tags, "minecraft:logs").expect("logs");
        assert_eq!(logs.last().map(String::as_str), Some("stone"));
        assert!(logs.iter().any(|v| v == "oak_log"));
        // A built-in tag that includes `#minecraft:logs` sees the pack's addition too.
        let mineable = resolve(&tags, "minecraft:mineable/axe").expect("mineable/axe");
        assert!(mineable.iter().any(|v| v == "stone"));
        assert_eq!(resolve(&tags, "test:own"), Some(logs));
    }

    #[test]
    fn replace_drops_builtin_entries_and_missing_references_fail_the_tag() {
        let builders = HashMap::from([
            (
                "minecraft:logs".to_string(),
                builder(&["minecraft:stone"], true),
            ),
            (
                "test:broken".to_string(),
                builder(&["minecraft:stone", "minecraft:not_a_block"], false),
            ),
            (
                "test:optional".to_string(),
                builder(
                    &["minecraft:stone", "minecraft:not_a_block?", "#test:none?"],
                    false,
                ),
            ),
            (
                "test:needs_broken".to_string(),
                builder(&["#test:broken"], false),
            ),
        ]);
        let tags = build_registry_tags(Some(RegistryKey::Block), &builders, &block_id);
        assert_eq!(
            resolve(&tags, "minecraft:logs"),
            Some(&["stone".to_string()][..])
        );
        assert_eq!(resolve(&tags, "test:broken"), None);
        assert_eq!(resolve(&tags, "test:needs_broken"), None);
        assert_eq!(
            resolve(&tags, "test:optional"),
            Some(&["stone".to_string()][..])
        );
    }

    #[test]
    fn builtin_tags_rebuild_unchanged() {
        let dynamic = DynamicRegistryManager::default();
        let mut wrong = Vec::new();
        for &key in RegistryKey::ALL {
            let Some(element) = element_lookup(key.identifier_string(), &dynamic) else {
                continue;
            };
            let tags = build_registry_tags(Some(key), &HashMap::new(), &element);
            let builtin = get_latest_map(key);
            if tags.len() != builtin.len() {
                wrong.push(format!("{key:?}: {} of {} tags", tags.len(), builtin.len()));
            }
            for (name, values, ids) in &tags {
                let tag = builtin.get(name.as_str()).expect("built-in tag");
                if tag.1 != ids.as_slice() || tag.0 != values.as_slice() {
                    wrong.push(format!("{key:?} {name}"));
                }
            }
        }
        assert!(wrong.is_empty(), "{wrong:?}");
    }

    #[test]
    fn tag_files_parse_like_vanilla() {
        let (entries, replace) = parse_tag_file(
            r##"{"replace": true, "values": ["stone", "#minecraft:logs", {"id": "c:x", "required": false}]}"##,
            "file/p",
        )
        .expect("parses");
        assert!(replace);
        let shown: Vec<String> = entries.iter().map(ToString::to_string).collect();
        assert_eq!(
            shown,
            [
                "minecraft:stone (from file/p)",
                "#minecraft:logs (from file/p)",
                "c:x? (from file/p)"
            ]
        );
        assert!(parse_tag_file(r#"{"replace": false}"#, "file/p").is_err());
        assert!(parse_tag_file(r#"{"values": ["Not Valid"]}"#, "file/p").is_err());
    }
}
