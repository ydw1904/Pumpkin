pub mod advancement_loader;
pub mod context_provider_loader;
pub mod damage_type_loader;
pub mod dynamic_registry_loader;
pub mod dynamic_registry_manager;
pub mod function_loader;
pub mod item_modifier_loader;
pub mod loot_table_loader;
pub mod predicate_loader;
pub mod recipe_loader;
pub mod slot_source_loader;
pub mod structure_loader;
pub mod tag_loader;
pub mod test_loader;
pub mod trade_loader;

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use tracing::{debug, error, info, warn};

use pumpkin_data::damage::DamageType;
use pumpkin_data::registry::RegistryEntryData;
use pumpkin_data::tag::RegistryKey;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_nbt::{NbtCompound, nbt_compress::read_gzip_compound_tag};
use pumpkin_protocol::codec::recipe::DynamicRecipe;
use pumpkin_protocol::java::client::config::RegistryTags;
use pumpkin_util::loot_table::DynamicLootTable;
use pumpkin_world::world_info::DataPacks;

use crate::command::context::command_source::{CommandSource, ResultValueTaker};
use crate::command::execution::{Action, Entry, ExecutionContext};
use crate::data::advancement_tree::ServerAdvancements;
use crate::server::Server;
use crate::server::recipe::RecipeManager;
use crate::world::loot::LootTableHandle;

use self::context_provider_loader::ContextProviderRegistry;
use self::damage_type_loader::{DamageTypeDefinition, DamageTypeEntry, DamageTypeRegistry};
use self::test_loader::{
    TestInstance, TestInstanceRegistry, load_test_instances_from_dir, to_registry_entry,
};
use self::trade_loader::{DynamicVillagerTradeSet, TradeRegistry};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnownPackData {
    pub namespace: String,
    pub id: String,
    pub version: String,
}

#[derive(Clone, Debug)]
pub struct LoadedDatapack {
    pub id: String,
    pub name: String,
    pub description: String,
    pub pack_format: u32,
    pub root_path: PathBuf,
    pub recipe_count: usize,
    pub function_count: usize,
    pub known_packs: Vec<KnownPackData>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DatapackInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub pack_format: u32,
    pub is_enabled: bool,
    pub recipe_count: usize,
    pub function_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DatapackEnablePosition {
    First,
    Last,
    Before(String),
    After(String),
}

pub struct DatapackManager {
    loaded_packs: RwLock<Vec<LoadedDatapack>>,
    functions: RwLock<HashMap<String, Arc<[String]>>>,
    function_tags: RwLock<HashMap<String, Vec<String>>>,
    test_instances: RwLock<TestInstanceRegistry>,
    context_int_providers: RwLock<ContextProviderRegistry>,
    context_float_providers: RwLock<ContextProviderRegistry>,
    damage_types: RwLock<DamageTypeRegistry>,
    loot_tables: RwLock<HashMap<String, Arc<DynamicLootTable>>>,
    dynamic_registries: RwLock<HashMap<String, HashMap<String, RegistryEntryData>>>,
    pub dynamic_registry_manager: Arc<dynamic_registry_manager::DynamicRegistryManager>,
    trade_registry: RwLock<TradeRegistry>,
    predicates: RwLock<HashMap<String, serde_json::Value>>,
    item_modifiers: RwLock<HashMap<String, serde_json::Value>>,
    slot_sources: RwLock<HashMap<String, serde_json::Value>>,
    /// Tags of synchronized registries without built-in tags (`wolf_variant`, ...), which only
    /// the client uses. The other registries' tags live in [`pumpkin_data::datapack_tags`].
    client_only_tags: RwLock<Vec<RegistryTags>>,
    /// The pack folders of the world's `datapacks/` as last scanned (vanilla
    /// `PackRepository.available`).
    world_packs: RwLock<Vec<String>>,
}

/// A function's id and its commands as loaded.
pub type FunctionBody = (String, Arc<[String]>);

pub const LOOT_TABLE_REGISTRY: &str = "minecraft:loot_table";
pub const PREDICATE_REGISTRY: &str = "minecraft:predicate";
pub const CONTEXT_FLOAT_PROVIDER_REGISTRY: &str = "minecraft:context_float_provider";
pub const CONTEXT_INT_PROVIDER_REGISTRY: &str = "minecraft:context_int_provider";
pub const ITEM_MODIFIER_REGISTRY: &str = "minecraft:item_modifier";
pub const SLOT_SOURCE_REGISTRY: &str = "minecraft:slot_source";
pub const ADVANCEMENT_REGISTRY: &str = "minecraft:advancement";
pub const RECIPE_REGISTRY: &str = "minecraft:recipe";

/// The elements of reloadable registries (vanilla `RegistryDataLoader.RELOADABLE_REGISTRIES`)
/// that failed to load, by registry and element id. Any of them fails the whole load.
#[derive(Default, Debug)]
pub struct RegistryLoadErrors(BTreeMap<(&'static str, String), String>);

impl RegistryLoadErrors {
    /// Records an element that failed to load. The same element from a later pack replaces
    /// the error, like vanilla only reads the file of the topmost pack.
    pub fn fail(&mut self, registry: &'static str, id: String, error: impl std::fmt::Display) {
        self.0.insert((registry, id), error.to_string());
    }

    /// Records an element that loaded, which hides the error of the same element in an
    /// earlier pack.
    pub fn loaded(&mut self, registry: &'static str, id: &str) {
        if !self.0.is_empty() {
            self.0.remove(&(registry, id.to_string()));
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[must_use]
    pub fn get(&self, registry: &'static str, id: &str) -> Option<&str> {
        self.0.get(&(registry, id.to_string())).map(String::as_str)
    }

    /// Vanilla `RegistryDataLoader.printFullDetailsToLog`.
    pub fn log(&self) {
        use std::fmt::Write;
        let mut report = String::new();
        let mut registry = None;
        for ((element_registry, id), error) in &self.0 {
            if registry != Some(*element_registry) {
                registry = Some(*element_registry);
                let _ = writeln!(report, "> Errors in registry {element_registry}:");
            }
            let _ = writeln!(report, ">> Errors in element {id}:\n{error}");
        }
        error!("Registry loading errors:\n{report}");
    }
}

/// Records an element Pumpkin's parser refused. A file that isn't a JSON object fails the load
/// like in vanilla. A JSON object is likely valid in vanilla but uses something Pumpkin can't
/// represent yet, so it's logged and left out rather than failing the whole load.
fn refuse_element(
    errors: &mut RegistryLoadErrors,
    registry: &'static str,
    id: String,
    content: &str,
) {
    match serde_json::from_str::<serde_json::Value>(content) {
        Ok(serde_json::Value::Object(_)) => {
            warn!("Pumpkin can't load {registry} element {id} yet, leaving it out");
            errors.loaded(registry, &id);
        }
        Ok(_) => errors.fail(registry, id, "Not a JSON object"),
        Err(error) => errors.fail(registry, id, error),
    }
}

/// The loot registries of the loaded packs, for loot data parsed after loading, like an item
/// modifier given to a command.
impl pumpkin_util::loot_table::parser::LootResolver for DatapackManager {
    fn predicate(&self, id: &str) -> Option<serde_json::Value> {
        self.get_predicate(id)
            .or_else(|| pumpkin_data::loot_table::get_loot_predicate(id).cloned())
    }

    fn item_modifier(&self, id: &str) -> Option<serde_json::Value> {
        self.get_item_modifier(id)
    }

    fn int_provider(&self, id: &str) -> Option<serde_json::Value> {
        self.get_context_int_provider(id)
            .map(|tag| crate::world::loot::nbt_to_json(&tag))
    }

    fn float_provider(&self, id: &str) -> Option<serde_json::Value> {
        self.get_context_float_provider(id)
            .map(|tag| crate::world::loot::nbt_to_json(&tag))
    }

    fn slot_source(&self, id: &str) -> Option<serde_json::Value> {
        self.get_slot_source(id)
    }
}

impl Default for DatapackManager {
    fn default() -> Self {
        Self::new()
    }
}

impl DatapackManager {
    #[must_use]
    pub fn new() -> Self {
        Self {
            loaded_packs: RwLock::new(Vec::new()),
            functions: RwLock::new(HashMap::new()),
            function_tags: RwLock::new(HashMap::new()),
            test_instances: RwLock::new(HashMap::new()),
            context_int_providers: RwLock::new(HashMap::new()),
            context_float_providers: RwLock::new(HashMap::new()),
            damage_types: RwLock::new(HashMap::new()),
            loot_tables: RwLock::new(HashMap::new()),
            dynamic_registries: RwLock::new(HashMap::new()),
            dynamic_registry_manager: Arc::new(
                dynamic_registry_manager::DynamicRegistryManager::new(
                    pumpkin_data::packet::CURRENT_MC_VERSION,
                ),
            ),
            trade_registry: RwLock::new(TradeRegistry::new()),
            predicates: RwLock::new(HashMap::new()),
            item_modifiers: RwLock::new(HashMap::new()),
            slot_sources: RwLock::new(HashMap::new()),
            client_only_tags: RwLock::new(Vec::new()),
            world_packs: RwLock::new(Vec::new()),
        }
    }

    /// Loads the enabled packs. Like vanilla `ReloadableServerRegistries.reload`, an element of
    /// a reloadable registry that fails to load fails the whole load: the errors are logged
    /// and returned, and the data loaded before stays. Otherwise this returns the advancements
    /// to use from now on.
    #[allow(clippy::too_many_lines)]
    pub fn load_all(
        &self,
        world_path: &Path,
        enabled_packs: &[String],
        recipe_manager: &RecipeManager,
        compile: function_loader::CommandCompiler,
    ) -> Result<ServerAdvancements, RegistryLoadErrors> {
        let datapacks_dir = world_path.join("datapacks");
        let mut errors = RegistryLoadErrors::default();
        let mut worldgen = structure_loader::StagedWorldgen::default();
        let mut loaded_packs_vec = Vec::new();
        let mut all_recipes: Vec<DynamicRecipe> = Vec::new();
        let mut all_functions: HashMap<String, String> = HashMap::new();
        let mut all_function_tags: HashMap<String, Vec<String>> = HashMap::new();
        let mut all_test_instances: TestInstanceRegistry = HashMap::new();
        let mut all_context_int_providers: ContextProviderRegistry = HashMap::new();
        let mut all_context_float_providers: ContextProviderRegistry = HashMap::new();
        let mut all_damage_type_defs: HashMap<String, DamageTypeDefinition> = HashMap::new();
        let mut all_loot_table_sources: HashMap<String, String> = HashMap::new();

        // Embedded test instances are compile-time constants and always load,
        // so on-disk packs can override them by id.
        let embedded_count = test_loader::load_embedded_test_instances(&mut all_test_instances);
        if embedded_count > 0 {
            info!("Loaded {embedded_count} embedded test instance(s)");
        }
        let (embedded_ints, embedded_floats) =
            context_provider_loader::load_embedded_context_providers(
                &mut all_context_int_providers,
                &mut all_context_float_providers,
            );
        if embedded_ints > 0 || embedded_floats > 0 {
            info!(
                "Loaded {embedded_ints} int and {embedded_floats} float embedded context provider(s)"
            );
        }
        let mut all_dynamic_registries: HashMap<String, HashMap<String, RegistryEntryData>> =
            HashMap::new();
        let mut all_trade_registry = TradeRegistry::new();
        let mut all_advancements: HashMap<
            String,
            advancement_loader::DynamicAdvancementDefinition,
        > = HashMap::new();
        let mut all_predicates: HashMap<String, serde_json::Value> = HashMap::new();
        let mut all_item_modifiers: HashMap<String, serde_json::Value> = HashMap::new();
        let mut all_slot_sources: HashMap<String, serde_json::Value> = HashMap::new();
        let mut all_tags = tag_loader::TagBuilders::new();

        let mut acc = PackContentAccumulators {
            recipes: &mut all_recipes,
            functions: &mut all_functions,
            function_tags: &mut all_function_tags,
            test_instances: &mut all_test_instances,
            damage_types: &mut all_damage_type_defs,
            context_int_providers: &mut all_context_int_providers,
            context_float_providers: &mut all_context_float_providers,
            loot_tables: &mut all_loot_table_sources,
            dynamic_registries: &mut all_dynamic_registries,
            trade_registry: &mut all_trade_registry,
            advancements: &mut all_advancements,
            predicates: &mut all_predicates,
            item_modifiers: &mut all_item_modifiers,
            slot_sources: &mut all_slot_sources,
            tags: &mut all_tags,
            worldgen: &mut worldgen,
            errors: &mut errors,
        };

        load_enabled_packs(
            &datapacks_dir,
            enabled_packs,
            &mut acc,
            &mut loaded_packs_vec,
        );

        // Loot tables refer to the predicates, item modifiers, number providers and slot
        // sources of every pack.
        let all_loot_tables = loot_table_loader::parse_loot_tables(
            &all_loot_table_sources,
            &loot_table_loader::PackLootRegistries {
                predicates: &all_predicates,
                item_modifiers: &all_item_modifiers,
                int_providers: &all_context_int_providers,
                float_providers: &all_context_float_providers,
                slot_sources: &all_slot_sources,
            },
            &mut errors,
        );

        let advancement_definitions: HashMap<String, serde_json::Value> = all_advancements
            .into_iter()
            .map(|(id, definition)| (id, definition.json))
            .collect();
        let advancements = ServerAdvancements::new(&advancement_definitions, &mut errors);
        if !errors.is_empty() {
            errors.log();
            return Err(errors);
        }

        worldgen.apply();
        let damage_type_registry = build_damage_type_registry(all_damage_type_defs);

        recipe_manager.set_recipes(all_recipes);
        *self
            .loaded_packs
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = loaded_packs_vec;
        *self
            .function_tags
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = all_function_tags;
        *self
            .test_instances
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = all_test_instances;
        *self
            .context_int_providers
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = all_context_int_providers;
        *self
            .context_float_providers
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = all_context_float_providers;
        for (id, entry) in &damage_type_registry {
            all_dynamic_registries
                .entry("minecraft:damage_type".to_string())
                .or_default()
                .insert(id.clone(), entry.to_registry_entry());
        }
        self.dynamic_registry_manager
            .apply_custom_registries(&all_dynamic_registries);

        *self
            .damage_types
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = damage_type_registry;
        *self
            .loot_tables
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = all_loot_tables;
        *self
            .dynamic_registries
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = all_dynamic_registries;

        *self
            .trade_registry
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = all_trade_registry;
        *self
            .predicates
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = all_predicates;
        *self
            .item_modifiers
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = all_item_modifiers;
        *self
            .slot_sources
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = all_slot_sources;
        // After the dynamic registries, so tags can hold the entries the packs add.
        self.bind_tags(&all_tags);
        // Last, so the commands can refer to everything else the packs define.
        let all_functions = function_loader::compile_functions(all_functions, compile);
        *self
            .functions
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = all_functions;
        Ok(advancements)
    }

    /// Resolves the packs' tags of every registry they touch and binds them (vanilla
    /// `TagLoader.loadTagsForExistingRegistries` and `PendingTags.apply`).
    fn bind_tags(&self, builders: &tag_loader::TagBuilders) {
        let mut bound = Vec::new();
        let mut client_only = Vec::new();
        let mut registries: Vec<_> = builders.iter().collect();
        registries.sort_unstable_by_key(|(registry, _)| *registry);
        for (registry, tags) in registries {
            let Some(element) =
                tag_loader::element_lookup(registry, &self.dynamic_registry_manager)
            else {
                debug!("Ignoring the tags of registry {registry}, which Pumpkin doesn't know");
                continue;
            };
            let key = RegistryKey::from_string(registry);
            let loaded = tag_loader::build_registry_tags(key, tags, &element);
            if let Some(key) = key {
                bound.push((key, loaded));
            } else {
                client_only.push(RegistryTags {
                    registry: format!("minecraft:{registry}").into(),
                    tags: loaded
                        .into_iter()
                        .map(|(name, _, ids)| (name.into(), ids.into()))
                        .collect(),
                });
            }
        }
        pumpkin_data::datapack_tags::bind(bound);
        *self
            .client_only_tags
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = client_only;
    }

    /// The tags `ClientboundUpdateTagsPacket` carries (vanilla
    /// `TagNetworkSerialization.serializeTagsToNetwork`): those of every synchronized registry.
    #[must_use]
    pub fn network_tags(&self) -> Vec<RegistryTags> {
        let mut registries: Vec<RegistryTags> = RegistryKey::NETWORK_KEYS
            .iter()
            .map(|&key| RegistryTags {
                registry: format!("minecraft:{}", key.identifier_string()).into(),
                tags: pumpkin_data::datapack_tags::tags(key)
                    .into_iter()
                    .map(|(name, tag)| (name.into(), tag.1.into()))
                    .collect(),
            })
            .collect();
        registries.extend(
            self.client_only_tags
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .iter()
                .cloned(),
        );
        registries.retain(|registry| !registry.tags.is_empty());
        registries
    }

    #[must_use]
    pub fn get_predicate(&self, id: &str) -> Option<serde_json::Value> {
        let guard = self
            .predicates
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let full_id = if id.contains(':') {
            id.to_string()
        } else {
            format!("minecraft:{id}")
        };
        guard
            .get(&full_id)
            .or_else(|| guard.get(id))
            .cloned()
            .or_else(|| {
                pumpkin_data::loot_table::get_loot_predicate(&full_id)
                    .or_else(|| pumpkin_data::loot_table::get_loot_predicate(id))
                    .cloned()
            })
    }

    #[must_use]
    pub fn get_predicate_ids(&self) -> Vec<String> {
        let guard = self
            .predicates
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut names: Vec<_> = guard.keys().cloned().collect();
        names.sort_unstable();
        names
    }

    #[must_use]
    pub fn get_item_modifier(&self, id: &str) -> Option<serde_json::Value> {
        let guard = self
            .item_modifiers
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let full_id = if id.contains(':') {
            id.to_string()
        } else {
            format!("minecraft:{id}")
        };
        guard.get(&full_id).or_else(|| guard.get(id)).cloned()
    }

    #[must_use]
    pub fn get_item_modifier_ids(&self) -> Vec<String> {
        let guard = self
            .item_modifiers
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut names: Vec<_> = guard.keys().cloned().collect();
        names.sort_unstable();
        names
    }

    #[must_use]
    pub fn get_slot_source(&self, id: &str) -> Option<serde_json::Value> {
        let guard = self
            .slot_sources
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let full_id = if id.contains(':') {
            id.to_string()
        } else {
            format!("minecraft:{id}")
        };
        guard.get(&full_id).or_else(|| guard.get(id)).cloned()
    }

    #[must_use]
    pub fn get_slot_source_ids(&self) -> Vec<String> {
        let guard = self
            .slot_sources
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut names: Vec<_> = guard.keys().cloned().collect();
        names.sort_unstable();
        names
    }

    pub fn get_loaded_packs(&self) -> Vec<LoadedDatapack> {
        self.loaded_packs
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn get_functions(&self) -> HashMap<String, Vec<String>> {
        self.functions
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .map(|(name, lines)| (name.clone(), lines.to_vec()))
            .collect()
    }

    pub fn get_test_instance(&self, name: &str) -> Option<TestInstance> {
        self.test_instances
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(name)
            .cloned()
    }

    pub fn get_test_instance_names(&self) -> Vec<String> {
        let test_instances = self
            .test_instances
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut names: Vec<_> = test_instances.keys().cloned().collect();
        names.sort_unstable();
        names
    }

    #[must_use]
    pub fn get_context_int_provider(&self, id: &str) -> Option<NbtTag> {
        self.context_int_providers
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(id)
            .cloned()
    }

    #[must_use]
    pub fn get_context_float_provider(&self, id: &str) -> Option<NbtTag> {
        self.context_float_providers
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(id)
            .cloned()
    }

    #[must_use]
    pub fn get_context_int_provider_names(&self) -> Vec<String> {
        let providers = self
            .context_int_providers
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut names: Vec<_> = providers.keys().cloned().collect();
        names.sort_unstable();
        names
    }

    #[must_use]
    pub fn get_context_float_provider_names(&self) -> Vec<String> {
        let providers = self
            .context_float_providers
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut names: Vec<_> = providers.keys().cloned().collect();
        names.sort_unstable();
        names
    }

    pub fn insert_context_int_provider(&self, id: String, tag: NbtTag) {
        self.context_int_providers
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(id, tag);
    }

    pub fn insert_context_float_provider(&self, id: String, tag: NbtTag) {
        self.context_float_providers
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(id, tag);
    }

    #[must_use]
    pub fn get_loot_table(&self, key: &str) -> Option<LootTableHandle> {
        let full_key = if key.contains(':') {
            key.to_string()
        } else {
            format!("minecraft:{key}")
        };

        let guard = self
            .loot_tables
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        if let Some(table) = guard.get(&full_key).or_else(|| guard.get(key)) {
            return Some(LootTableHandle::Dynamic(table.clone()));
        }

        pumpkin_data::loot_table::get_loot_table(key)
            .or_else(|| pumpkin_data::loot_table::get_loot_table(&full_key))
            .map(LootTableHandle::Static)
    }

    #[must_use]
    pub fn get_loot_table_names(&self) -> Vec<String> {
        let guard = self
            .loot_tables
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut names: Vec<_> = guard.keys().cloned().collect();
        names.sort_unstable();
        names
    }

    pub fn insert_loot_table(&self, key: String, table: Arc<DynamicLootTable>) {
        self.loot_tables
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(key, table);
    }

    #[must_use]
    pub fn get_damage_type(&self, id: &str) -> Option<DamageType> {
        let guard = self
            .damage_types
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let full_id = if id.contains(':') {
            id.to_string()
        } else {
            format!("minecraft:{id}")
        };
        guard
            .get(&full_id)
            .or_else(|| guard.get(id))
            .map(|e| e.damage_type)
    }

    #[must_use]
    pub fn get_damage_type_names(&self) -> Vec<String> {
        let guard = self
            .damage_types
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut names: Vec<_> = guard.keys().cloned().collect();
        names.sort_unstable();
        names
    }

    #[must_use]
    pub fn get_damage_type_registry_map(&self) -> HashMap<String, DamageTypeEntry> {
        self.damage_types
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    #[must_use]
    pub fn get_all_damage_type_registry_entries(&self) -> Vec<RegistryEntryData> {
        let guard = self
            .damage_types
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut entries: Vec<RegistryEntryData> = guard
            .values()
            .map(DamageTypeEntry::to_registry_entry)
            .collect();
        entries.sort_by(|a, b| a.entry_id.cmp(&b.entry_id));
        entries
    }

    #[must_use]
    pub fn merge_damage_type_entries(
        &self,
        vanilla_entries: &[RegistryEntryData],
    ) -> Vec<RegistryEntryData> {
        let guard = self
            .damage_types
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        merge_damage_type_entries(vanilla_entries, &guard)
    }

    #[must_use]
    pub fn merge_registry_entries(
        &self,
        registry_id: &str,
        vanilla_entries: &[RegistryEntryData],
    ) -> Vec<RegistryEntryData> {
        let guard = self
            .dynamic_registries
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let custom = guard
            .get(registry_id)
            .or_else(|| guard.get(&format!("minecraft:{registry_id}")))
            .or_else(|| {
                registry_id
                    .strip_prefix("minecraft:")
                    .and_then(|r| guard.get(r))
            });
        custom.map_or_else(
            || vanilla_entries.iter().map(clone_registry_entry).collect(),
            |custom| {
                dynamic_registry_loader::merge_dynamic_registry_entries(vanilla_entries, custom)
            },
        )
    }

    #[must_use]
    pub fn get_custom_registry_entry(
        &self,
        registry_id: &str,
        entry_id: &str,
    ) -> Option<RegistryEntryData> {
        let guard = self
            .dynamic_registries
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let custom_map = guard
            .get(registry_id)
            .or_else(|| guard.get(&format!("minecraft:{registry_id}")))
            .or_else(|| {
                registry_id
                    .strip_prefix("minecraft:")
                    .and_then(|r| guard.get(r))
            })?;
        custom_map.get(entry_id).map(clone_registry_entry)
    }

    #[must_use]
    pub fn get_custom_registry_entries(&self, registry_id: &str) -> Vec<RegistryEntryData> {
        let guard = self
            .dynamic_registries
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let custom_map = guard
            .get(registry_id)
            .or_else(|| guard.get(&format!("minecraft:{registry_id}")))
            .or_else(|| {
                registry_id
                    .strip_prefix("minecraft:")
                    .and_then(|r| guard.get(r))
            });
        custom_map.map_or_else(Vec::new, |entries| {
            let mut list: Vec<_> = entries.values().map(clone_registry_entry).collect();
            list.sort_by(|a, b| a.entry_id.cmp(&b.entry_id));
            list
        })
    }

    #[must_use]
    pub fn get_dynamic_registry_ids(&self) -> Vec<String> {
        let guard = self
            .dynamic_registries
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut ids: Vec<_> = guard.keys().cloned().collect();
        ids.sort();
        ids
    }

    #[must_use]
    pub fn get_dynamic_registry_raw_id(&self, registry_id: &str, entry_id: &str) -> Option<u32> {
        self.dynamic_registry_manager
            .get_raw_id(registry_id, entry_id)
    }

    #[must_use]
    pub fn get_dynamic_registry_entry_id(&self, registry_id: &str, raw_id: u32) -> Option<String> {
        self.dynamic_registry_manager
            .get_entry_id(registry_id, raw_id)
    }

    #[must_use]
    pub fn get_dynamic_registry_entry_data(
        &self,
        registry_id: &str,
        entry_id: &str,
    ) -> Option<Box<[u8]>> {
        self.dynamic_registry_manager
            .get_entry_data(registry_id, entry_id)
    }

    #[must_use]
    pub fn get_dynamic_registry_entries(&self, registry_id: &str) -> Vec<RegistryEntryData> {
        self.dynamic_registry_manager
            .get_registry_entries(registry_id)
    }

    #[must_use]
    pub fn has_dynamic_registry_entry(&self, registry_id: &str, entry_id: &str) -> bool {
        self.dynamic_registry_manager
            .contains_entry(registry_id, entry_id)
    }

    #[must_use]
    pub fn get_all_synced_registries(&self) -> Vec<pumpkin_data::registry::Registry> {
        self.dynamic_registry_manager.get_synced_registries()
    }

    #[must_use]
    pub fn get_villager_trade_set(
        &self,
        profession: &str,
        level: i32,
    ) -> Option<DynamicVillagerTradeSet> {
        let registry = self
            .trade_registry
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        registry.get_villager_trade_set(profession, level)
    }

    #[must_use]
    pub fn get_wandering_trader_trade_set(&self, tier: &str) -> Option<DynamicVillagerTradeSet> {
        let registry = self
            .trade_registry
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        registry.get_wandering_trader_trade_set(tier)
    }

    #[must_use]
    pub fn get_trade_set(&self, key: &str) -> Option<DynamicVillagerTradeSet> {
        let registry = self
            .trade_registry
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        registry.resolve_trade_set(key)
    }

    /// Returns datapack test instances in the protocol's synced-registry entry format.
    /// The vanilla Test Instance Block renderer resolves required/padding/base rotation
    /// through this registry using the controller's `data.test` resource key.
    pub fn get_test_instance_registry_entries(&self) -> Vec<RegistryEntryData> {
        let test_instances = self
            .test_instances
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut entries: Vec<_> = test_instances
            .iter()
            .map(|(id, instance)| to_registry_entry(id.clone(), instance))
            .collect();
        entries.sort_unstable_by(|left, right| left.entry_id.cmp(&right.entry_id));
        entries
    }

    /// Returns the list of known packs for client configuration / pack synchronization,
    /// matching vanilla's `server.getResourceManager().listPacks().flatMap(p -> p.location().knownPackInfo().stream())`.
    pub fn get_known_packs<'a>(
        &self,
        server: &Server,
        server_version: &'a str,
        loaded_packs: &'a [LoadedDatapack],
    ) -> Vec<pumpkin_protocol::KnownPack<'a>> {
        use pumpkin_protocol::KnownPack;

        let mut known_packs = Vec::new();

        // 1. Primary vanilla core pack
        known_packs.push(KnownPack {
            namespace: "minecraft",
            id: "core",
            version: server_version,
        });

        // 2. Built-in feature packs that are enabled
        let enabled_packs = Self::get_enabled_packs(server);
        for pack_name in &enabled_packs {
            let id: &'static str = match pack_name.as_str() {
                "trade_rebalance" => "trade_rebalance",
                "minecart_improvements" => "minecart_improvements",
                "redstone_experiments" => "redstone_experiments",
                "bundle" => "bundle",
                _ => continue,
            };
            let pack = KnownPack {
                namespace: "minecraft",
                id,
                version: server_version,
            };
            if !known_packs
                .iter()
                .any(|p| p.namespace == pack.namespace && p.id == pack.id)
            {
                known_packs.push(pack);
            }
        }

        // 3. Loaded packs with known_pack info from pack.mcmeta
        for pack in loaded_packs {
            for kp in &pack.known_packs {
                let p = KnownPack {
                    namespace: &kp.namespace,
                    id: &kp.id,
                    version: &kp.version,
                };
                if !known_packs.iter().any(|existing| {
                    existing.namespace == p.namespace
                        && existing.id == p.id
                        && existing.version == p.version
                }) {
                    known_packs.push(p);
                }
            }
        }

        known_packs
    }

    /// Returns the enabled world feature flags (e.g. `minecraft:vanilla`, `minecraft:trade_rebalance`,
    /// `minecraft:minecart_improvements`, `minecraft:redstone_experiments`, `minecraft:bundle`).
    #[must_use]
    pub fn get_enabled_features(&self, server: &Server) -> Vec<&'static str> {
        let enabled_packs = Self::get_enabled_packs(server);
        Self::resolve_enabled_features(&enabled_packs)
    }

    /// Resolves enabled feature flag names given a list of enabled pack names.
    #[must_use]
    pub fn resolve_enabled_features(enabled_packs: &[String]) -> Vec<&'static str> {
        let mut features = vec!["minecraft:vanilla"];

        for pack_name in enabled_packs {
            let feature: &'static str = match pack_name.as_str() {
                "trade_rebalance" | "file/trade_rebalance" => "minecraft:trade_rebalance",
                "minecart_improvements" | "file/minecart_improvements" => {
                    "minecraft:minecart_improvements"
                }
                "redstone_experiments" | "file/redstone_experiments" => {
                    "minecraft:redstone_experiments"
                }
                "bundle" | "file/bundle" => "minecraft:bundle",
                _ => {
                    if let Some(stripped) = pack_name.strip_prefix("file/") {
                        match stripped {
                            "trade_rebalance" => "minecraft:trade_rebalance",
                            "minecart_improvements" => "minecraft:minecart_improvements",
                            "redstone_experiments" => "minecraft:redstone_experiments",
                            "bundle" => "minecraft:bundle",
                            _ => continue,
                        }
                    } else {
                        continue;
                    }
                }
            };
            if !features.contains(&feature) {
                features.push(feature);
            }
        }

        features
    }

    #[must_use]
    pub fn is_feature_enabled(&self, server: &Server, feature: &str) -> bool {
        self.get_enabled_features(server).contains(&feature)
    }

    /// Loads a Java Edition structure NBT from the currently enabled datapacks.
    ///
    /// Structure identifiers are resource locations such as
    /// `minecraft:village/plains/houses/plains_small_house_1`. Both the current
    /// `structure` directory and the legacy `structures` directory are checked.
    pub async fn load_structure(&self, resource_location: &str) -> Result<NbtCompound, String> {
        let (namespace, path) = parse_structure_resource_location(resource_location)?;

        let nbt_path = {
            let loaded_packs = self
                .loaded_packs
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);

            let mut nbt_path = None;

            // Runtime datapacks override embedded resources.
            'packs: for pack in loaded_packs.iter().rev() {
                for structure_dir in ["structure", "structures"] {
                    let candidate = pack
                        .root_path
                        .join("data")
                        .join(namespace)
                        .join(structure_dir)
                        .join(format!("{path}.nbt"));

                    if candidate.is_file() {
                        nbt_path = Some(candidate);
                        break 'packs;
                    }
                }
            }

            nbt_path
        };

        if let Some(nbt_path) = nbt_path {
            let display_path = nbt_path.display().to_string();

            return tokio::task::spawn_blocking(move || {
                let file = fs::File::open(&nbt_path).map_err(|error| {
                    format!("Failed to open structure '{display_path}': {error}")
                })?;

                read_gzip_compound_tag(file)
                    .map_err(|error| format!("Failed to parse structure '{display_path}': {error}"))
            })
            .await
            .map_err(|error| format!("Structure loader task failed: {error}"))?;
        }

        // Fall back to compile-time embedded structures.
        let structure_id = format!("{namespace}:{path}");

        if let Some(template) =
            pumpkin_world::generation::structure::template::get_template(&structure_id)
        {
            return Ok(template.save());
        }

        Err(format!(
            "Structure '{resource_location}' was not found in any enabled datapack or embedded resources"
        ))
    }

    pub fn get_function_names(&self) -> Vec<String> {
        let fns = self
            .functions
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tags = self
            .function_tags
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut names = Vec::with_capacity(fns.len() + tags.len());
        names.extend(fns.keys().cloned());
        for tag in tags.keys() {
            names.push(format!("#{tag}"));
        }
        names
    }

    /// Returns the functions a `#tag` name expands to, or `None` for a plain function name or an
    /// unknown tag.
    pub fn function_tag_members(&self, name: &str) -> Option<Vec<String>> {
        let tag_name = name.strip_prefix('#')?;
        self.function_tags
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(tag_name)
            .cloned()
    }

    /// Runs a function or every function in a `#tag` from the game loop (vanilla
    /// `ServerFunctionManager.execute`), each in its own command queue and with output
    /// suppressed. Returns how much of the command limit they used.
    pub fn execute_function(
        &self,
        server: &Arc<Server>,
        source: &CommandSource,
        name: &str,
    ) -> Result<usize, String> {
        let source = source
            .clone()
            .with_silent()
            .with_command_result_taker(ResultValueTaker::new());
        let mut executed = 0;
        for (id, lines) in self.resolve_functions(name)? {
            let lines = match function_loader::instantiate_function(&id, &lines, None) {
                Ok(lines) => lines,
                Err(reason) => {
                    warn!("Failed to run function {id}: {}", reason.get_text());
                    continue;
                }
            };
            executed += ExecutionContext::execute_in_context(server, |context| {
                context.queue_next(Entry {
                    frame: context.top_frame(ResultValueTaker::new()),
                    action: Action::CallFunction {
                        source: Box::new(source.clone()),
                        lines,
                        callback: ResultValueTaker::new(),
                        return_parent: false,
                    },
                });
            });
        }
        Ok(executed)
    }

    /// The functions a function name or `#tag` stands for, with their commands. A tag skips
    /// entries that aren't loaded.
    pub fn resolve_functions(&self, name: &str) -> Result<Vec<FunctionBody>, String> {
        let (ids, is_tag) = if let Some(tag_name) = name.strip_prefix('#') {
            let tags = self
                .function_tags
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(ids) = tags.get(tag_name) else {
                return Err(format!("Unknown function tag: #{tag_name}"));
            };
            (ids.clone(), true)
        } else {
            (vec![name.to_string()], false)
        };

        let all_functions = self
            .functions
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut functions = Vec::with_capacity(ids.len());
        for id in ids {
            match all_functions.get(&id) {
                Some(lines) => functions.push((id, Arc::clone(lines))),
                None if is_tag => {}
                None => return Err(format!("Unknown function: {id}")),
            }
        }
        Ok(functions)
    }

    #[must_use]
    pub fn is_embedded_pack(name: &str) -> bool {
        pumpkin_world::generation::structure::template::all_embedded_datapack_names()
            .contains(&name)
    }

    /// Vanilla `PackRepository.getAvailableIds`: the embedded and bundled packs and the world's
    /// pack folders as last scanned, not what's on disk right now.
    pub fn get_all_known_packs(server: &Server) -> Vec<String> {
        let mut packs: Vec<String> =
            pumpkin_world::generation::structure::template::all_embedded_datapack_names()
                .iter()
                .map(|name| (*name).to_owned())
                .collect();

        // Bundled feature packs that are known to the server independently of
        // the compile-time structure/test resource embeddings.
        for bundled in BUNDLED_PACKS {
            if !packs.iter().any(|p| p == bundled) {
                packs.push(bundled.to_string());
            }
        }

        for pack_name in server
            .datapack_manager
            .world_packs
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
        {
            if !packs.contains(pack_name) {
                packs.push(pack_name.clone());
            }
        }

        packs
    }

    /// The selected packs, without the ones the last scan didn't find (vanilla
    /// `PackRepository.rebuildSelected`).
    pub fn get_enabled_packs(server: &Server) -> Vec<String> {
        // Compile-time embedded datapacks cannot be disabled at runtime, so
        // expose them as permanently enabled.
        let mut packs: Vec<String> =
            pumpkin_world::generation::structure::template::all_embedded_datapack_names()
                .iter()
                .map(|name| (*name).to_owned())
                .collect();

        let known = Self::get_all_known_packs(server);
        for pack in &server.level_info.load().data_packs.enabled {
            if known.contains(pack) && !packs.iter().any(|existing| existing == pack) {
                packs.push(pack.clone());
            }
        }

        packs
    }

    /// The packs the world lists as enabled that the last scan didn't find.
    #[must_use]
    pub fn missing_packs(server: &Server) -> Vec<String> {
        let known = Self::get_all_known_packs(server);
        server
            .level_info
            .load()
            .data_packs
            .enabled
            .iter()
            .filter(|pack| !known.contains(pack))
            .cloned()
            .collect()
    }

    pub fn get_available_packs(server: &Server) -> Vec<String> {
        let enabled = Self::get_enabled_packs(server);
        let all = Self::get_all_known_packs(server);
        all.into_iter().filter(|p| !enabled.contains(p)).collect()
    }

    pub fn find_pack_name(server: &Server, input: &str) -> Option<String> {
        let known = Self::get_all_known_packs(server);
        if let Some(p) = known.iter().find(|p| *p == input) {
            return Some(p.clone());
        }
        let file_input = format!("file/{input}");
        if let Some(p) = known.iter().find(|p| **p == file_input) {
            return Some(p.clone());
        }
        if let Some(p) = known
            .iter()
            .find(|p| p.strip_prefix("file/") == Some(input))
        {
            return Some(p.clone());
        }
        None
    }

    pub fn get_pack_info(server: &Server, name_or_id: &str) -> Option<DatapackInfo> {
        let resolved_name = Self::find_pack_name(server, name_or_id)?;
        let enabled_packs = Self::get_enabled_packs(server);
        let is_enabled = enabled_packs.contains(&resolved_name);

        let loaded = server.datapack_manager.get_loaded_packs();
        if let Some(pack) = loaded
            .iter()
            .find(|p| p.id == resolved_name || p.name == resolved_name)
        {
            return Some(DatapackInfo {
                id: pack.id.clone(),
                name: pack.name.clone(),
                description: pack.description.clone(),
                pack_format: pack.pack_format,
                is_enabled,
                recipe_count: pack.recipe_count,
                function_count: pack.function_count,
            });
        }

        let (id, name, description, pack_format) = if resolved_name == "vanilla" {
            (
                "vanilla".to_string(),
                "vanilla".to_string(),
                "The default data pack".to_string(),
                61,
            )
        } else if Self::is_embedded_pack(&resolved_name) {
            (
                resolved_name.clone(),
                resolved_name.clone(),
                format!("Embedded datapack: {resolved_name}"),
                61,
            )
        } else if let Some(stripped) = resolved_name.strip_prefix("file/") {
            let pack_path = server
                .basic_config
                .get_world_path()
                .join("datapacks")
                .join(stripped);
            let (desc, format, _) = read_pack_mcmeta(&pack_path);
            (resolved_name.clone(), stripped.to_string(), desc, format)
        } else {
            (
                resolved_name.clone(),
                resolved_name.clone(),
                format!("Bundled datapack: {resolved_name}"),
                61,
            )
        };

        Some(DatapackInfo {
            id,
            name,
            description,
            pack_format,
            is_enabled,
            recipe_count: 0,
            function_count: 0,
        })
    }

    pub fn list_all_packs(server: &Server) -> Vec<DatapackInfo> {
        let all = Self::get_all_known_packs(server);
        all.into_iter()
            .filter_map(|p| Self::get_pack_info(server, &p))
            .collect()
    }

    pub fn list_enabled_packs(server: &Server) -> Vec<DatapackInfo> {
        let enabled = Self::get_enabled_packs(server);
        enabled
            .into_iter()
            .filter_map(|p| Self::get_pack_info(server, &p))
            .collect()
    }

    pub fn list_available_packs(server: &Server) -> Vec<DatapackInfo> {
        let available = Self::get_available_packs(server);
        available
            .into_iter()
            .filter_map(|p| Self::get_pack_info(server, &p))
            .collect()
    }

    pub fn is_pack_enabled(server: &Server, name: &str) -> bool {
        let Some(resolved) = Self::find_pack_name(server, name) else {
            return false;
        };
        Self::get_enabled_packs(server).contains(&resolved)
    }

    /// Enables a pack and reloads; the world's selection only changes if the reload succeeds.
    pub fn enable_pack(
        server: &Arc<Server>,
        name: &str,
        position: DatapackEnablePosition,
    ) -> Result<(), String> {
        let Some(target) = Self::find_pack_name(server, name) else {
            return Err(format!("Unknown datapack '{name}'"));
        };

        let enabled = Self::get_enabled_packs(server);
        if enabled.contains(&target) {
            return Err(format!("Datapack '{target}' is already enabled"));
        }

        let mut data_packs = server.level_info.load().data_packs.clone();
        // Embedded packs aren't in the world's list; a pack placed next to one goes last.
        let index_of = |existing_name: &str| {
            let Some(existing_pack) = Self::find_pack_name(server, existing_name) else {
                return Err(format!("Unknown existing datapack '{existing_name}'"));
            };
            if !enabled.contains(&existing_pack) {
                return Err(format!("Datapack '{existing_pack}' is not enabled"));
            }
            Ok(data_packs.enabled.iter().position(|p| p == &existing_pack))
        };
        let len = data_packs.enabled.len();
        let index = match position {
            DatapackEnablePosition::First => 0,
            DatapackEnablePosition::Last => len,
            DatapackEnablePosition::Before(existing_name) => {
                index_of(&existing_name)?.unwrap_or(len)
            }
            DatapackEnablePosition::After(existing_name) => {
                index_of(&existing_name)?.map_or(len, |index| index + 1)
            }
        };
        data_packs.disabled.retain(|p| p != &target);
        data_packs.enabled.insert(index, target);
        Self::reload_with(server, &data_packs)
    }

    /// Disables a pack and reloads; the world's selection only changes if the reload succeeds.
    pub fn disable_pack(server: &Arc<Server>, name: &str) -> Result<(), String> {
        let Some(target) = Self::find_pack_name(server, name) else {
            return Err(format!("Unknown datapack '{name}'"));
        };

        if !Self::get_enabled_packs(server).contains(&target) {
            return Err(format!("Datapack '{target}' is not enabled"));
        }

        if Self::is_embedded_pack(&target) {
            return Err(format!(
                "Cannot disable embedded datapack '{target}' because it is compiled into the server"
            ));
        }

        let mut data_packs = server.level_info.load().data_packs.clone();
        data_packs.enabled.retain(|p| p != &target);
        if !data_packs.disabled.contains(&target) {
            data_packs.disabled.push(target);
        }
        Self::reload_with(server, &data_packs)
    }

    /// Vanilla `ReloadCommand.discoverNewPacks`: the current selection plus the packs added to
    /// `datapacks/` since they were last looked at, unless the world lists them as disabled.
    #[must_use]
    pub fn discover_new_packs(server: &Server) -> DataPacks {
        server.datapack_manager.rescan_world_packs(server);
        let mut data_packs = server.level_info.load().data_packs.clone();
        let known = Self::get_all_known_packs(server);
        // `MinecraftServer.getSelectedPacks` keeps only packs the repository has.
        data_packs.enabled.retain(|pack| known.contains(pack));
        data_packs.disabled.retain(|pack| known.contains(pack));
        let new_packs: Vec<String> = known
            .into_iter()
            .filter(|pack| {
                pack.starts_with("file/")
                    && !data_packs.enabled.contains(pack)
                    && !data_packs.disabled.contains(pack)
            })
            .collect();
        data_packs.enabled.extend(new_packs);
        data_packs
    }

    /// Vanilla `PackRepository.reload`: looks at the world's `datapacks/` folder again. Until
    /// then, packs added there are unknown.
    pub fn rescan_world_packs(&self, server: &Server) {
        let mut packs = Vec::new();
        let datapacks_dir = server.basic_config.get_world_path().join("datapacks");
        if let Ok(entries) = fs::read_dir(datapacks_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let file_name = entry.file_name().to_string_lossy().to_string();
                if !file_name.starts_with('.')
                    && (path.is_dir()
                        || path
                            .extension()
                            .is_some_and(|ext| ext.eq_ignore_ascii_case("zip")))
                {
                    packs.push(format!("file/{file_name}"));
                }
            }
        }
        // `discoverAvailable` collects the packs into a `TreeMap`.
        packs.sort_unstable();
        *self
            .world_packs
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = packs;
    }

    pub fn reload(server: &Arc<Server>) -> Result<(), String> {
        let data_packs = server.level_info.load().data_packs.clone();
        Self::reload_with(server, &data_packs)
    }

    fn reload_with(server: &Arc<Server>, data_packs: &DataPacks) -> Result<(), String> {
        server
            .reload_datapacks(data_packs)
            .map_err(|_| "Reload failed; keeping old data".to_string())
    }

    pub fn execute_function_from_console(
        server: &Arc<Server>,
        name: &str,
    ) -> Result<usize, String> {
        let source = crate::command::CommandSender::Console.into_source(server);
        server
            .datapack_manager
            .execute_function(server, &source, name)
    }
}

fn parse_structure_resource_location(resource_location: &str) -> Result<(&str, &str), String> {
    let (namespace, raw_path) = resource_location
        .split_once(':')
        .unwrap_or(("minecraft", resource_location));

    if namespace.is_empty()
        || !namespace.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-' | b'.')
        })
    {
        return Err(format!(
            "Invalid structure namespace in '{resource_location}'"
        ));
    }

    let path = raw_path.strip_suffix(".nbt").unwrap_or(raw_path);
    if path.is_empty()
        || path
            .split('/')
            .any(|segment| segment.is_empty() || segment == "." || segment == "..")
        || !path.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'_' | b'-' | b'.' | b'/')
        })
    {
        return Err(format!("Invalid structure path in '{resource_location}'"));
    }

    Ok((namespace, path))
}

fn build_damage_type_registry(
    mut all_damage_type_defs: HashMap<String, DamageTypeDefinition>,
) -> DamageTypeRegistry {
    let mut damage_type_registry: DamageTypeRegistry = HashMap::new();
    let mut next_custom_id = 51u8;
    let mut static_map: HashMap<String, &'static DamageType> = HashMap::new();
    let mut names = Vec::new();

    let mut sorted_ids: Vec<String> = all_damage_type_defs.keys().cloned().collect();
    sorted_ids.sort();

    for id in sorted_ids {
        let Some(def) = all_damage_type_defs.remove(&id) else {
            continue;
        };
        let path = id.split_once(':').map_or(id.as_str(), |(_, p)| p);
        let numeric_id = pumpkin_data::damage::DamageType::from_name(path)
            .or_else(|| pumpkin_data::damage::DamageType::from_name(&id))
            .map_or_else(
                || {
                    let nid = next_custom_id;
                    next_custom_id = next_custom_id.saturating_add(1);
                    nid
                },
                |vanilla_dt| vanilla_dt.id,
            );

        let dt = damage_type_loader::to_damage_type(numeric_id, &def);
        let nbt_data = damage_type_loader::to_registry_nbt_bytes(&def);
        let leaked_dt = Box::leak(Box::new(dt));
        static_map.insert(id.clone(), leaked_dt);
        static_map.insert(path.to_string(), leaked_dt);
        names.push(id.clone());

        damage_type_registry.insert(
            id.clone(),
            DamageTypeEntry {
                id,
                numeric_id,
                definition: def,
                damage_type: dt,
                nbt_data,
            },
        );
    }

    pumpkin_command::argument_types::resource::register_dynamic_damage_types(
        Box::new(move |req_id: &str| static_map.get(req_id).copied()),
        names,
    );

    damage_type_registry
}

struct PackContentAccumulators<'a> {
    recipes: &'a mut Vec<DynamicRecipe>,
    functions: &'a mut HashMap<String, String>,
    function_tags: &'a mut HashMap<String, Vec<String>>,
    test_instances: &'a mut TestInstanceRegistry,
    damage_types: &'a mut HashMap<String, DamageTypeDefinition>,
    context_int_providers: &'a mut ContextProviderRegistry,
    context_float_providers: &'a mut ContextProviderRegistry,
    /// The JSON of each loot table, parsed once every pack is loaded.
    loot_tables: &'a mut HashMap<String, String>,
    dynamic_registries: &'a mut HashMap<String, HashMap<String, RegistryEntryData>>,
    trade_registry: &'a mut TradeRegistry,
    advancements: &'a mut HashMap<String, advancement_loader::DynamicAdvancementDefinition>,
    predicates: &'a mut HashMap<String, serde_json::Value>,
    item_modifiers: &'a mut HashMap<String, serde_json::Value>,
    slot_sources: &'a mut HashMap<String, serde_json::Value>,
    tags: &'a mut tag_loader::TagBuilders,
    worldgen: &'a mut structure_loader::StagedWorldgen,
    errors: &'a mut RegistryLoadErrors,
}

#[derive(Default, Clone, Copy)]
struct PackContentCounts {
    recipes: usize,
    functions: usize,
    test_instances: usize,
    damage_types: usize,
    int_providers: usize,
    float_providers: usize,
    loot_tables: usize,
    structures: usize,
    template_pools: usize,
    processor_lists: usize,
    dynamic_registries: usize,
    trade_sets: usize,
    trades: usize,
    trade_tags: usize,
    advancements: usize,
    predicates: usize,
    item_modifiers: usize,
    slot_sources: usize,
    tags: usize,
}

fn find_bundled_datapack_dir(pack_name: &str) -> Option<PathBuf> {
    let sub = format!("assets/datapack/data/minecraft/datapacks/{pack_name}");
    let candidates = [
        PathBuf::from(&sub),
        PathBuf::from(format!("../{sub}")),
        PathBuf::from(format!("../../{sub}")),
    ];
    for candidate in candidates {
        if candidate.is_dir() {
            return Some(candidate);
        }
    }
    if let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") {
        let path = Path::new(&manifest_dir).join(format!("../../{sub}"));
        if path.is_dir() {
            return Some(path);
        }
    }
    if let Ok(exe_path) = std::env::current_exe()
        && let Some(exe_dir) = exe_path.parent()
    {
        for ancestor in exe_dir.ancestors().take(5) {
            let path = ancestor.join(&sub);
            if path.is_dir() {
                return Some(path);
            }
        }
    }
    None
}

/// The feature packs Pumpkin ships in `assets/datapack`.
const BUNDLED_PACKS: [&str; 3] = [
    "trade_rebalance",
    "minecart_improvements",
    "redstone_experiments",
];

/// Loads the enabled packs in the order of the enabled list, so a later pack's files replace an
/// earlier pack's (vanilla `MultiPackResourceManager`). A world pack folder wins over a bundled
/// pack of the same name.
fn load_enabled_packs(
    datapacks_dir: &Path,
    enabled_packs: &[String],
    acc: &mut PackContentAccumulators<'_>,
    loaded_packs_vec: &mut Vec<LoadedDatapack>,
) {
    for entry in enabled_packs {
        let file_name = entry.strip_prefix("file/").unwrap_or(entry);
        let is_folder_name = !file_name.starts_with('.')
            && matches!(
                Path::new(file_name).components().collect::<Vec<_>>()[..],
                [std::path::Component::Normal(_)]
            );
        let world_pack = datapacks_dir.join(file_name);
        let (name, pack_id, pack_path, kind) = if is_folder_name && world_pack.is_dir() {
            (
                file_name,
                format!("file/{file_name}"),
                world_pack,
                "datapack",
            )
        } else {
            let name = file_name.strip_prefix("minecraft:").unwrap_or(file_name);
            let Some(pack_path) = BUNDLED_PACKS
                .contains(&name)
                .then(|| find_bundled_datapack_dir(name))
                .flatten()
            else {
                continue;
            };
            (
                name,
                format!("minecraft:{name}"),
                pack_path,
                "bundled datapack",
            )
        };
        if loaded_packs_vec.iter().any(|pack| pack.id == pack_id) {
            continue;
        }
        let name = name.to_string();

        let (description, pack_format, known_packs) = read_pack_mcmeta(&pack_path);
        let counts = load_pack_contents(&pack_path, &pack_id, acc);
        info!(
            "Loaded {kind} '{name}': {} recipe(s), {} function(s), {} test instance(s), {} damage type(s), {} int provider(s), {} float provider(s), {} loot table(s), {} structure(s), {} template pool(s), {} processor list(s), {} dynamic registry entry/entries, {} trade set(s), {} trade(s), {} advancement(s), {} predicate(s), {} tag(s)",
            counts.recipes,
            counts.functions,
            counts.test_instances,
            counts.damage_types,
            counts.int_providers,
            counts.float_providers,
            counts.loot_tables,
            counts.structures,
            counts.template_pools,
            counts.processor_lists,
            counts.dynamic_registries,
            counts.trade_sets,
            counts.trades,
            counts.advancements,
            counts.predicates,
            counts.tags,
        );
        loaded_packs_vec.push(LoadedDatapack {
            id: pack_id,
            name,
            description,
            pack_format,
            root_path: pack_path,
            recipe_count: counts.recipes,
            function_count: counts.functions,
            known_packs,
        });
    }
}

/// Loads recipes, functions, function tags, test instances, damage types, context number providers,
/// and loot tables from a single datapack directory, returning per-pack counts as [`PackContentCounts`].
#[allow(clippy::too_many_lines)]
fn load_pack_contents(
    pack_path: &Path,
    pack_id: &str,
    acc: &mut PackContentAccumulators<'_>,
) -> PackContentCounts {
    let data_dir = pack_path.join("data");
    let mut counts = PackContentCounts::default();

    if data_dir.is_dir()
        && let Ok(ns_entries) = fs::read_dir(&data_dir)
    {
        for ns_entry in ns_entries.flatten() {
            let ns_path = ns_entry.path();
            if !ns_path.is_dir() {
                continue;
            }
            let namespace = ns_entry.file_name().to_string_lossy().to_string();

            // Load recipes
            for recipe_sub in ["recipe", "recipes"] {
                let recipe_dir = ns_path.join(recipe_sub);
                if recipe_dir.is_dir() {
                    load_recipes_from_dir(
                        &namespace,
                        &recipe_dir,
                        acc.recipes,
                        &mut counts.recipes,
                        acc.errors,
                    );
                }
            }

            // Load functions
            for fn_sub in ["function", "functions"] {
                let fn_dir = ns_path.join(fn_sub);
                if fn_dir.is_dir() {
                    let before = acc.functions.len();
                    function_loader::load_functions_from_dir(&namespace, &fn_dir, acc.functions);
                    counts.functions += acc.functions.len() - before;
                }
            }

            // Load tags
            let tags_dir = ns_path.join("tags");
            if tags_dir.is_dir() {
                function_loader::load_function_tags_from_dir(
                    &namespace,
                    &tags_dir,
                    acc.function_tags,
                );
                counts.tags +=
                    tag_loader::load_tags_from_dir(&namespace, &tags_dir, pack_id, acc.tags);
            }
            // Load game test instances
            let test_instance_dir = ns_path.join("test_instance");
            if test_instance_dir.is_dir() {
                counts.test_instances += load_test_instances_from_dir(
                    &namespace,
                    &test_instance_dir,
                    acc.test_instances,
                );
            }

            // Load damage types
            for dt_sub in ["damage_type", "damage_types"] {
                let dt_dir = ns_path.join(dt_sub);
                if dt_dir.is_dir() {
                    counts.damage_types += damage_type_loader::load_damage_types_from_dir(
                        &namespace,
                        &dt_dir,
                        acc.damage_types,
                    );
                }
            }

            // Load context int providers
            for int_sub in ["context_int_provider", "context_int_providers"] {
                let provider_dir = ns_path.join(int_sub);
                if provider_dir.is_dir() {
                    counts.int_providers +=
                        context_provider_loader::load_context_providers_from_dir(
                            &namespace,
                            &provider_dir,
                            acc.context_int_providers,
                            CONTEXT_INT_PROVIDER_REGISTRY,
                            acc.errors,
                        );
                }
            }

            // Load context float providers
            for float_sub in ["context_float_provider", "context_float_providers"] {
                let provider_dir = ns_path.join(float_sub);
                if provider_dir.is_dir() {
                    counts.float_providers +=
                        context_provider_loader::load_context_providers_from_dir(
                            &namespace,
                            &provider_dir,
                            acc.context_float_providers,
                            CONTEXT_FLOAT_PROVIDER_REGISTRY,
                            acc.errors,
                        );
                }
            }

            // Load loot tables
            for lt_sub in ["loot_table", "loot_tables"] {
                let lt_dir = ns_path.join(lt_sub);
                if lt_dir.is_dir() {
                    counts.loot_tables += loot_table_loader::load_loot_tables_from_dir(
                        &namespace,
                        &lt_dir,
                        acc.loot_tables,
                        acc.errors,
                    );
                }
            }

            // Load advancements
            for adv_sub in ["advancement", "advancements"] {
                let adv_dir = ns_path.join(adv_sub);
                if adv_dir.is_dir() {
                    counts.advancements += advancement_loader::load_advancements_from_dir(
                        &namespace,
                        &adv_dir,
                        acc.advancements,
                        acc.errors,
                    );
                }
            }

            // Load predicates
            for pred_sub in ["predicate", "predicates"] {
                let pred_dir = ns_path.join(pred_sub);
                if pred_dir.is_dir() {
                    counts.predicates += predicate_loader::load_predicates_from_dir(
                        &namespace,
                        &pred_dir,
                        acc.predicates,
                        acc.errors,
                    );
                }
            }

            // Load item modifiers
            for im_sub in ["item_modifier", "item_modifiers"] {
                let im_dir = ns_path.join(im_sub);
                if im_dir.is_dir() {
                    counts.item_modifiers += item_modifier_loader::load_item_modifiers_from_dir(
                        &namespace,
                        &im_dir,
                        acc.item_modifiers,
                        acc.errors,
                    );
                }
            }

            // Load slot sources
            for ss_sub in ["slot_source", "slot_sources"] {
                let ss_dir = ns_path.join(ss_sub);
                if ss_dir.is_dir() {
                    counts.slot_sources += slot_source_loader::load_slot_sources_from_dir(
                        &namespace,
                        &ss_dir,
                        acc.slot_sources,
                        acc.errors,
                    );
                }
            }

            // Load structure templates
            for st_sub in ["structure", "structures"] {
                let st_dir = ns_path.join(st_sub);
                if st_dir.is_dir() {
                    counts.structures += structure_loader::load_structures_from_dir(
                        &namespace,
                        &st_dir,
                        acc.worldgen,
                    );
                }
            }

            // Load worldgen template pools and processor lists
            let worldgen_dir = ns_path.join("worldgen");
            if worldgen_dir.is_dir() {
                let pool_dir = worldgen_dir.join("template_pool");
                if pool_dir.is_dir() {
                    counts.template_pools += structure_loader::load_template_pools_from_dir(
                        &namespace,
                        &pool_dir,
                        acc.worldgen,
                    );
                }
                let proc_dir = worldgen_dir.join("processor_list");
                if proc_dir.is_dir() {
                    counts.processor_lists += structure_loader::load_processor_lists_from_dir(
                        &namespace,
                        &proc_dir,
                        acc.worldgen,
                    );
                }
            }

            // Load dynamic registry entries (wolf_variant, cat_variant, frog_variant, instrument,
            // trim_material, trim_pattern, banner_pattern, decorated_pot_pattern, chat_type, etc.)
            counts.dynamic_registries += dynamic_registry_loader::load_dynamic_registries_from_ns(
                &namespace,
                &ns_path,
                acc.dynamic_registries,
            );

            // Load trade sets
            for ts_sub in ["trade_set", "trade_sets"] {
                let ts_dir = ns_path.join(ts_sub);
                if ts_dir.is_dir() {
                    counts.trade_sets += trade_loader::load_trade_sets_from_dir(
                        &namespace,
                        &ts_dir,
                        &mut acc.trade_registry.trade_sets,
                    );
                }
            }

            // Load villager trades
            for vt_sub in ["villager_trade", "villager_trades"] {
                let vt_dir = ns_path.join(vt_sub);
                if vt_dir.is_dir() {
                    counts.trades += trade_loader::load_trades_from_dir(
                        &namespace,
                        &vt_dir,
                        &mut acc.trade_registry.trades,
                    );
                }
            }

            // Load villager trade tags
            let tags_dir = ns_path.join("tags");
            if tags_dir.is_dir() {
                for vt_tag_sub in ["villager_trade", "villager_trades"] {
                    let vt_tag_dir = tags_dir.join(vt_tag_sub);
                    if vt_tag_dir.is_dir() {
                        counts.trade_tags += trade_loader::load_trade_tags_from_dir(
                            &namespace,
                            &vt_tag_dir,
                            &mut acc.trade_registry.trade_tags,
                        );
                    }
                }
            }
        }
    }

    counts
}

#[must_use]
pub fn clone_registry_entry(entry: &RegistryEntryData) -> RegistryEntryData {
    RegistryEntryData {
        entry_id: entry.entry_id.clone(),
        data: entry.data.clone(),
    }
}

#[must_use]
pub fn merge_damage_type_entries<S: std::hash::BuildHasher>(
    vanilla_entries: &[RegistryEntryData],
    custom_entries: &HashMap<String, DamageTypeEntry, S>,
) -> Vec<RegistryEntryData> {
    if custom_entries.is_empty() {
        return vanilla_entries.iter().map(clone_registry_entry).collect();
    }

    let mut merged = Vec::with_capacity(vanilla_entries.len() + custom_entries.len());
    let mut overridden = std::collections::HashSet::new();

    for entry in vanilla_entries {
        let full_id = if entry.entry_id.contains(':') {
            entry.entry_id.clone()
        } else {
            format!("minecraft:{}", entry.entry_id)
        };

        if let Some(custom) = custom_entries
            .get(&full_id)
            .or_else(|| custom_entries.get(&entry.entry_id))
        {
            let mut reg_entry = custom.to_registry_entry();
            reg_entry.entry_id.clone_from(&entry.entry_id);
            merged.push(reg_entry);
            overridden.insert(full_id);
            overridden.insert(entry.entry_id.clone());
        } else {
            merged.push(clone_registry_entry(entry));
        }
    }

    let mut sorted_custom: Vec<&DamageTypeEntry> = custom_entries
        .values()
        .filter(|e| !overridden.contains(&e.id))
        .collect();
    sorted_custom.sort_by_key(|e| e.numeric_id);

    for custom in sorted_custom {
        merged.push(custom.to_registry_entry());
    }

    merged
}

fn read_pack_mcmeta(pack_path: &Path) -> (String, u32, Vec<KnownPackData>) {
    let mcmeta_path = pack_path.join("pack.mcmeta");
    if let Ok(content) = fs::read_to_string(mcmeta_path)
        && let Ok(val) = serde_json::from_str::<serde_json::Value>(&content)
    {
        let pack = val.get("pack");
        let description = pack
            .and_then(|p| p.get("description"))
            .map(|d| {
                d.as_str()
                    .map_or_else(|| d.to_string(), ToString::to_string)
            })
            .unwrap_or_default();
        let pack_format = pack
            .and_then(|p| {
                p.get("pack_format")
                    .or_else(|| p.get("max_format"))
                    .or_else(|| p.get("min_format"))
            })
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(61) as u32;

        let mut known_packs = Vec::new();
        let parse_item = |item: &serde_json::Value| -> Option<KnownPackData> {
            let ns = item.get("namespace").and_then(serde_json::Value::as_str)?;
            let id = item.get("id").and_then(serde_json::Value::as_str)?;
            let ver = item.get("version").and_then(serde_json::Value::as_str)?;
            Some(KnownPackData {
                namespace: ns.to_string(),
                id: id.to_string(),
                version: ver.to_string(),
            })
        };

        if let Some(packs_array) = val.get("known_packs").and_then(serde_json::Value::as_array) {
            for item in packs_array {
                if let Some(kp) = parse_item(item) {
                    known_packs.push(kp);
                }
            }
        } else if let Some(item) = val.get("known_pack") {
            if let Some(kp) = parse_item(item) {
                known_packs.push(kp);
            }
        } else if let Some(pack_obj) = pack {
            if let Some(packs_array) = pack_obj
                .get("known_packs")
                .and_then(serde_json::Value::as_array)
            {
                for item in packs_array {
                    if let Some(kp) = parse_item(item) {
                        known_packs.push(kp);
                    }
                }
            } else if let Some(item) = pack_obj.get("known_pack")
                && let Some(kp) = parse_item(item)
            {
                known_packs.push(kp);
            }
        }

        return (description, pack_format, known_packs);
    }
    (String::new(), 61, Vec::new())
}

fn load_recipes_recursive(
    namespace: &str,
    base_dir: &Path,
    current_dir: &Path,
    all_recipes: &mut Vec<DynamicRecipe>,
    count: &mut usize,
    errors: &mut RegistryLoadErrors,
) {
    let Ok(entries) = fs::read_dir(current_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            load_recipes_recursive(namespace, base_dir, &path, all_recipes, count, errors);
        } else if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
        {
            let Ok(relative_path) = path.strip_prefix(base_dir) else {
                continue;
            };

            let recipe_name = relative_path
                .with_extension("")
                .to_string_lossy()
                .replace('\\', "/");

            let recipe_id = format!("{namespace}:{recipe_name}");
            let content = match fs::read_to_string(&path) {
                Ok(content) => content,
                Err(error) => {
                    errors.fail(RECIPE_REGISTRY, recipe_id, error);
                    continue;
                }
            };
            // A later pack's file replaces an earlier pack's recipe of the same id.
            all_recipes.retain(|recipe| recipe.recipe_id() != Some(recipe_id.as_str()));
            if let Some(recipe) = recipe_loader::parse_recipe(namespace, &recipe_name, &content) {
                errors.loaded(RECIPE_REGISTRY, &recipe_id);
                all_recipes.push(recipe);
                *count += 1;
            } else {
                refuse_element(errors, RECIPE_REGISTRY, recipe_id, &content);
            }
        }
    }
}

fn load_recipes_from_dir(
    namespace: &str,
    dir: &Path,
    all_recipes: &mut Vec<DynamicRecipe>,
    count: &mut usize,
    errors: &mut RegistryLoadErrors,
) {
    load_recipes_recursive(namespace, dir, dir, all_recipes, count, errors);
}
#[cfg(test)]
mod tests {
    use super::DatapackManager;

    #[test]
    fn function_dispatch_releases_the_functions_lock() {
        let manager = DatapackManager::new();
        manager
            .functions
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                "test:reentrant".to_string(),
                vec!["first command".to_string(), "second command".to_string()].into(),
            );

        let functions = manager
            .resolve_functions("test:reentrant")
            .expect("resolve function");
        drop(
            manager
                .functions
                .try_write()
                .expect("function resolution must not hold the functions read lock"),
        );
        assert_eq!(functions.len(), 1);
        assert_eq!(&*functions[0].1, ["first command", "second command"]);
    }

    #[test]
    fn function_resolution_preserves_unknown_errors_and_skips_missing_tag_entries() {
        let manager = DatapackManager::new();
        manager
            .functions
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                "test:known".to_string(),
                vec!["known command".to_string()].into(),
            );
        manager
            .function_tags
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                "test:mixed".to_string(),
                vec!["test:missing".to_string(), "test:known".to_string()],
            );

        let functions = manager
            .resolve_functions("#test:mixed")
            .expect("resolve function tag");
        assert_eq!(functions.len(), 1);
        assert_eq!(functions[0].0, "test:known");

        assert_eq!(
            manager
                .resolve_functions("test:missing")
                .expect_err("unknown function must fail"),
            "Unknown function: test:missing"
        );
        assert_eq!(
            manager
                .resolve_functions("#test:missing")
                .expect_err("unknown function tag must fail"),
            "Unknown function tag: #test:missing"
        );
    }

    #[test]
    fn read_pack_mcmeta_parses_known_packs() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let pack_path = temp_dir.path();

        // 1. Test root known_packs array
        let mcmeta_content = r#"{
            "pack": {
                "description": "Test Pack",
                "pack_format": 61
            },
            "known_packs": [
                {
                    "namespace": "example",
                    "id": "content",
                    "version": "1.0.0"
                }
            ]
        }"#;
        std::fs::write(pack_path.join("pack.mcmeta"), mcmeta_content).unwrap();

        let (desc, format, known_packs) = super::read_pack_mcmeta(pack_path);
        assert_eq!(desc, "Test Pack");
        assert_eq!(format, 61);
        assert_eq!(known_packs.len(), 1);
        assert_eq!(known_packs[0].namespace, "example");
        assert_eq!(known_packs[0].id, "content");
        assert_eq!(known_packs[0].version, "1.0.0");

        // 2. Test nested pack.known_packs
        let mcmeta_content_nested = r#"{
            "pack": {
                "description": "Nested Pack",
                "pack_format": 61,
                "known_packs": [
                    {
                        "namespace": "nested",
                        "id": "pack",
                        "version": "2.0.0"
                    }
                ]
            }
        }"#;
        std::fs::write(pack_path.join("pack.mcmeta"), mcmeta_content_nested).unwrap();

        let (_, _, known_packs) = super::read_pack_mcmeta(pack_path);
        assert_eq!(known_packs.len(), 1);
        assert_eq!(known_packs[0].namespace, "nested");
        assert_eq!(known_packs[0].id, "pack");
        assert_eq!(known_packs[0].version, "2.0.0");
    }

    #[test]
    fn resolve_enabled_features_works() {
        // Default only vanilla
        let default_features = DatapackManager::resolve_enabled_features(&[]);
        assert_eq!(default_features, ["minecraft:vanilla"]);

        // With minecart improvements
        let minecart_features =
            DatapackManager::resolve_enabled_features(&["file/minecart_improvements".to_string()]);
        assert_eq!(
            minecart_features,
            ["minecraft:vanilla", "minecraft:minecart_improvements"]
        );

        // Multiple experimental features
        let multi_features = DatapackManager::resolve_enabled_features(&[
            "trade_rebalance".to_string(),
            "redstone_experiments".to_string(),
            "file/bundle".to_string(),
        ]);
        assert_eq!(
            multi_features,
            [
                "minecraft:vanilla",
                "minecraft:trade_rebalance",
                "minecraft:redstone_experiments",
                "minecraft:bundle"
            ]
        );
    }

    #[test]
    fn vanilla_datapack_coverage_test() {
        let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let datapack_path = manifest_dir.join("../../assets/datapack");
        if !datapack_path.join("data/minecraft").is_dir() {
            return;
        }

        let mut recipes = Vec::new();
        let mut functions = std::collections::HashMap::new();
        let mut function_tags = std::collections::HashMap::new();
        let mut test_instances = std::collections::HashMap::new();
        let mut damage_types = std::collections::HashMap::new();
        let mut context_int_providers = std::collections::HashMap::new();
        let mut context_float_providers = std::collections::HashMap::new();
        let mut loot_tables = std::collections::HashMap::new();
        let mut dynamic_registries = std::collections::HashMap::new();
        let mut trade_registry = super::TradeRegistry::new();
        let mut advancements = std::collections::HashMap::new();
        let mut predicates = std::collections::HashMap::new();
        let mut item_modifiers = std::collections::HashMap::new();
        let mut slot_sources = std::collections::HashMap::new();
        let mut tags = std::collections::HashMap::new();
        let mut worldgen = super::structure_loader::StagedWorldgen::default();
        let mut errors = super::RegistryLoadErrors::default();

        let mut acc = super::PackContentAccumulators {
            recipes: &mut recipes,
            functions: &mut functions,
            function_tags: &mut function_tags,
            test_instances: &mut test_instances,
            damage_types: &mut damage_types,
            context_int_providers: &mut context_int_providers,
            context_float_providers: &mut context_float_providers,
            loot_tables: &mut loot_tables,
            dynamic_registries: &mut dynamic_registries,
            trade_registry: &mut trade_registry,
            advancements: &mut advancements,
            predicates: &mut predicates,
            item_modifiers: &mut item_modifiers,
            slot_sources: &mut slot_sources,
            tags: &mut tags,
            worldgen: &mut worldgen,
            errors: &mut errors,
        };

        let counts = super::load_pack_contents(&datapack_path, "vanilla", &mut acc);
        assert!(errors.is_empty(), "vanilla elements must load: {errors:?}");
        assert_eq!(counts.recipes, 2042, "all vanilla recipes must load");
        assert_eq!(
            counts.loot_tables, 1447,
            "all vanilla loot tables must load"
        );
        assert_eq!(
            counts.advancements, 1866,
            "all vanilla advancements must load"
        );
        assert_eq!(counts.structures, 1511, "all vanilla structures must load");
        assert_eq!(
            counts.template_pools, 245,
            "all vanilla template pools must load"
        );
        assert_eq!(
            counts.processor_lists, 40,
            "all vanilla processor lists must load"
        );
        assert_eq!(
            counts.damage_types, 51,
            "all vanilla damage types must load"
        );
        assert_eq!(
            counts.int_providers, 26,
            "all vanilla int providers must load"
        );
        assert_eq!(
            counts.float_providers, 4,
            "all vanilla float providers must load"
        );
        assert_eq!(
            counts.test_instances, 1,
            "all vanilla test instances must load"
        );
        assert!(
            counts.dynamic_registries >= 2080,
            "all vanilla dynamic registry entries must load"
        );
        assert!(counts.tags >= 1406, "all vanilla tags must load");
        assert_eq!(counts.predicates, 3, "all vanilla predicates must load");
    }

    #[test]
    fn bundled_trade_rebalance_coverage_test() {
        if let Some(pack_path) = super::find_bundled_datapack_dir("trade_rebalance") {
            let mut recipes = Vec::new();
            let mut functions = std::collections::HashMap::new();
            let mut function_tags = std::collections::HashMap::new();
            let mut test_instances = std::collections::HashMap::new();
            let mut damage_types = std::collections::HashMap::new();
            let mut context_int_providers = std::collections::HashMap::new();
            let mut context_float_providers = std::collections::HashMap::new();
            let mut loot_tables = std::collections::HashMap::new();
            let mut dynamic_registries = std::collections::HashMap::new();
            let mut trade_registry = super::TradeRegistry::new();
            let mut advancements = std::collections::HashMap::new();
            let mut predicates = std::collections::HashMap::new();
            let mut item_modifiers = std::collections::HashMap::new();
            let mut slot_sources = std::collections::HashMap::new();
            let mut tags = std::collections::HashMap::new();
            let mut worldgen = super::structure_loader::StagedWorldgen::default();
            let mut errors = super::RegistryLoadErrors::default();

            let mut acc = super::PackContentAccumulators {
                recipes: &mut recipes,
                functions: &mut functions,
                function_tags: &mut function_tags,
                test_instances: &mut test_instances,
                damage_types: &mut damage_types,
                context_int_providers: &mut context_int_providers,
                context_float_providers: &mut context_float_providers,
                loot_tables: &mut loot_tables,
                dynamic_registries: &mut dynamic_registries,
                trade_registry: &mut trade_registry,
                advancements: &mut advancements,
                predicates: &mut predicates,
                item_modifiers: &mut item_modifiers,
                slot_sources: &mut slot_sources,
                tags: &mut tags,
                worldgen: &mut worldgen,
                errors: &mut errors,
            };

            let counts = super::load_pack_contents(&pack_path, "trade_rebalance", &mut acc);
            assert!(
                errors.is_empty(),
                "trade rebalance elements must load: {errors:?}"
            );
            assert!(counts.trades > 0, "trade rebalance must load trades");
            assert!(
                counts.trade_tags > 0,
                "trade rebalance must load trade tags"
            );
            assert!(
                counts.loot_tables > 0,
                "trade rebalance must load loot tables"
            );
        }
    }

    #[test]
    fn datapack_manager_dynamic_registry_and_tag_test() {
        let manager = super::DatapackManager::new();
        assert!(
            manager
                .dynamic_registry_manager
                .get_synced_registries()
                .len()
                >= 30
        );
        let synced = manager.get_all_synced_registries();
        assert!(
            synced
                .iter()
                .any(|r| r.registry_id == "minecraft:painting_variant")
        );
        assert!(
            synced
                .iter()
                .any(|r| r.registry_id == "minecraft:wolf_variant")
        );
    }
}
