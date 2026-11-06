//! Vanilla `ServerAdvancementManager` and `AdvancementTree`: the built-in advancements with the
//! ones enabled datapacks add or override, and their layout (`TreeNodePosition`).

use crate::data::datapack::{ADVANCEMENT_REGISTRY, RegistryLoadErrors};
use crate::entity::player::advancement::predicate::CriterionConditions;
use pumpkin_data::advancement_data::{
    AdvancementCondition, AdvancementCriterion, AdvancementDisplay, AdvancementReward, FrameType,
};
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::{ADVANCEMENT_TREE, Advancement};
use pumpkin_util::identifier::Identifier;
use pumpkin_util::text::TextComponent;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use tracing::{error, info};

/// One advancement (vanilla `AdvancementHolder`) with its parsed criteria and the rewards the
/// built-in data doesn't carry.
#[derive(Clone)]
pub struct AdvancementEntry {
    pub advancement: &'static Advancement,
    /// The conditions of `advancement.criteria_definitions`, in the same order.
    pub conditions: Arc<[Arc<CriterionConditions>]>,
    /// Vanilla `AdvancementRewards.loot`.
    pub loot: Arc<[String]>,
    /// Vanilla `AdvancementRewards.function`.
    pub function: Option<Arc<str>>,
}

impl AdvancementEntry {
    fn built_in(advancement: &'static Advancement) -> Self {
        Self {
            advancement,
            conditions: advancement
                .criteria_definitions
                .iter()
                .map(|criterion| Arc::new(CriterionConditions::from(criterion.conditions)))
                .collect(),
            loot: Arc::new([]),
            function: None,
        }
    }

    /// The conditions of the named criterion.
    #[must_use]
    pub fn criterion_conditions(&self, name: &str) -> Option<&Arc<CriterionConditions>> {
        self.advancement
            .criteria_definitions
            .iter()
            .position(|criterion| criterion.name == name)
            .and_then(|index| self.conditions.get(index))
    }
}

/// Vanilla `AdvancementNode`.
pub struct TreeNode {
    pub entry: usize,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
    pub x: f32,
    pub y: f32,
}

/// Vanilla `ServerAdvancementManager` with its `AdvancementTree`.
pub struct ServerAdvancements {
    entries: Vec<AdvancementEntry>,
    by_id: HashMap<Identifier, usize>,
    nodes: Vec<TreeNode>,
    node_of_entry: Vec<Option<usize>>,
}

static BUILT_IN: LazyLock<Vec<AdvancementEntry>> = LazyLock::new(|| {
    ADVANCEMENT_TREE
        .nodes_vector
        .iter()
        .map(|node| AdvancementEntry::built_in(node.value))
        .collect()
});

/// Datapack advancements by id and definition, so reloading an unchanged pack reuses them.
static INTERNED: LazyLock<Mutex<HashMap<String, &'static Advancement>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

impl Default for ServerAdvancements {
    fn default() -> Self {
        Self::from_parsed(HashMap::new())
    }
}

/// The trigger types vanilla 26.3 registers in `CriteriaTriggers`.
const TRIGGER_TYPES: [&str; 58] = [
    "impossible",
    "player_killed_entity",
    "entity_killed_player",
    "enter_block",
    "inventory_changed",
    "recipe_unlocked",
    "player_hurt_entity",
    "entity_hurt_player",
    "enchanted_item",
    "filled_bucket",
    "brewed_potion",
    "construct_beacon",
    "used_ender_eye",
    "summoned_entity",
    "bred_animals",
    "location",
    "slept_in_bed",
    "cured_zombie_villager",
    "villager_trade",
    "item_durability_changed",
    "levitation",
    "changed_dimension",
    "tick",
    "tame_animal",
    "placed_block",
    "consume_item",
    "effects_changed",
    "used_totem",
    "nether_travel",
    "fishing_rod_hooked",
    "channeled_lightning",
    "shot_crossbow",
    "spear_mobs",
    "killed_by_arrow",
    "hero_of_the_village",
    "voluntary_exile",
    "slide_down_block",
    "bee_nest_destroyed",
    "target_hit",
    "item_used_on_block",
    "default_block_use",
    "any_block_use",
    "player_generates_container_loot",
    "thrown_item_picked_up_by_entity",
    "thrown_item_picked_up_by_player",
    "player_interacted_with_entity",
    "player_sheared_equipment",
    "started_riding",
    "lightning_strike",
    "using_item",
    "fall_from_height",
    "ride_entity_in_lava",
    "kill_mob_near_sculk_catalyst",
    "allay_drop_item_on_block",
    "avoid_vibration",
    "recipe_crafted",
    "crafter_recipe_crafted",
    "fall_after_explosion",
];

impl ServerAdvancements {
    /// Builds the advancements from the built-in ones and the datapack definitions by id. A
    /// datapack definition replaces the built-in advancement with its id. A definition that
    /// fails vanilla's `Advancement.CODEC` goes into `errors`, which fails the whole load.
    #[must_use]
    pub fn new<S: std::hash::BuildHasher>(
        datapack: &HashMap<String, Value, S>,
        errors: &mut RegistryLoadErrors,
    ) -> Self {
        let mut parsed = HashMap::new();
        for (id, json) in datapack {
            let Ok(identifier) = Identifier::parse(id) else {
                errors.fail(ADVANCEMENT_REGISTRY, id.clone(), "invalid id");
                continue;
            };
            match parse_advancement(&identifier, json) {
                Ok(entry) => {
                    parsed.insert(identifier, entry);
                }
                Err(reason) => errors.fail(ADVANCEMENT_REGISTRY, id.clone(), reason),
            }
        }
        let advancements = Self::from_parsed(parsed);
        info!("Loaded {} advancements", advancements.nodes.len());
        advancements
    }

    fn from_parsed(mut parsed: HashMap<Identifier, AdvancementEntry>) -> Self {
        // Built-in advancements keep the order the generated tree has; the ones datapacks add
        // follow in vanilla's registry order.
        let mut entries = Vec::new();
        for built_in in BUILT_IN.iter() {
            entries.push(
                parsed
                    .remove(&built_in.advancement.id)
                    .unwrap_or_else(|| built_in.clone()),
            );
        }
        let mut added: Vec<AdvancementEntry> = parsed.into_values().collect();
        added.sort_by(|a, b| {
            let (a, b) = (&a.advancement.id, &b.advancement.id);
            (a.path(), a.namespace()).cmp(&(b.path(), b.namespace()))
        });
        entries.extend(added);

        let by_id = entries
            .iter()
            .enumerate()
            .map(|(index, entry)| (entry.advancement.id.clone(), index))
            .collect();
        let mut advancements = Self {
            node_of_entry: vec![None; entries.len()],
            entries,
            by_id,
            nodes: Vec::new(),
        };
        advancements.add_all();
        advancements.reposition_nodes();
        advancements
    }

    /// Vanilla `AdvancementTree.addAll`.
    fn add_all(&mut self) {
        let mut to_add: Vec<usize> = (0..self.entries.len()).collect();
        while !to_add.is_empty() {
            let before = to_add.len();
            let mut remaining = Vec::new();
            for entry in to_add {
                if !self.try_insert(entry) {
                    remaining.push(entry);
                }
            }
            if remaining.len() == before {
                let ids: Vec<String> = remaining
                    .iter()
                    .map(|&entry| self.entries[entry].advancement.id.to_string())
                    .collect();
                error!("Couldn't load advancements: {ids:?}");
                break;
            }
            to_add = remaining;
        }
    }

    /// Vanilla `AdvancementTree.tryInsert`.
    fn try_insert(&mut self, entry: usize) -> bool {
        let parent = match &self.entries[entry].advancement.parent {
            Some(parent_id) => {
                let Some(parent) = self
                    .by_id
                    .get(parent_id)
                    .and_then(|&parent| self.node_of_entry[parent])
                else {
                    return false;
                };
                Some(parent)
            }
            None => None,
        };
        let node = self.nodes.len();
        self.nodes.push(TreeNode {
            entry,
            parent,
            children: Vec::new(),
            x: 0.0,
            y: 0.0,
        });
        if let Some(parent) = parent {
            self.nodes[parent].children.push(node);
        }
        self.node_of_entry[entry] = Some(node);
        true
    }

    /// Vanilla `AdvancementTree.repositionNodes`.
    fn reposition_nodes(&mut self) {
        for root in 0..self.nodes.len() {
            if self.nodes[root].parent.is_none() && self.node_advancement(root).display.is_some() {
                tree_node_position::run(self, root);
            }
        }
    }

    /// Every advancement, in or out of the tree (vanilla `getAllAdvancements`).
    #[must_use]
    pub fn entries(&self) -> &[AdvancementEntry] {
        &self.entries
    }

    #[must_use]
    pub fn get(&self, id: &Identifier) -> Option<&AdvancementEntry> {
        self.by_id.get(id).map(|&entry| &self.entries[entry])
    }

    /// The advancement with the given id, which defaults to the `minecraft` namespace.
    #[must_use]
    pub fn get_by_name(&self, id: &str) -> Option<&'static Advancement> {
        let id = Identifier::parse(id).ok()?;
        self.get(&id).map(|entry| entry.advancement)
    }

    /// The tree node of the advancement, if it made it into the tree.
    #[must_use]
    pub fn node_of(&self, id: &Identifier) -> Option<usize> {
        self.by_id
            .get(id)
            .and_then(|&entry| self.node_of_entry[entry])
    }

    #[must_use]
    pub fn node(&self, node: usize) -> &TreeNode {
        &self.nodes[node]
    }

    #[must_use]
    pub fn node_advancement(&self, node: usize) -> &'static Advancement {
        self.entries[self.nodes[node].entry].advancement
    }

    /// Vanilla `AdvancementNode.root`.
    #[must_use]
    pub fn root(&self, mut node: usize) -> usize {
        while let Some(parent) = self.nodes[node].parent {
            node = parent;
        }
        node
    }
}

fn leak_str(string: &str) -> &'static str {
    Box::leak(string.into())
}

fn get_bool(json: &Value, key: &str, default: bool) -> Result<bool, String> {
    json.get(key).map_or(Ok(default), |value| {
        value
            .as_bool()
            .ok_or_else(|| format!("{key} is not a boolean"))
    })
}

fn parse_identifier(value: &Value, what: &str) -> Result<Identifier, String> {
    value
        .as_str()
        .and_then(|id| Identifier::parse(id).ok())
        .ok_or_else(|| format!("invalid {what}: {value}"))
}

/// Vanilla `Advancement.CODEC`, including `Advancement.validate`.
fn parse_advancement(id: &Identifier, json: &Value) -> Result<AdvancementEntry, String> {
    let parent = json
        .get("parent")
        .map(|parent| parse_identifier(parent, "parent"))
        .transpose()?;

    let criteria_json = json
        .get("criteria")
        .and_then(Value::as_object)
        .ok_or("missing criteria")?;
    if criteria_json.is_empty() {
        return Err("Advancement criteria cannot be empty".to_string());
    }
    let mut criteria = Vec::with_capacity(criteria_json.len());
    for (name, criterion) in criteria_json {
        let trigger = criterion
            .get("trigger")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("criterion {name} has no trigger"))?;
        let trigger =
            Identifier::parse(trigger).map_err(|_| format!("invalid trigger {trigger}"))?;
        if trigger.namespace() != "minecraft" || !TRIGGER_TYPES.contains(&trigger.path()) {
            return Err(format!(
                "Unknown registry key in ResourceKey[minecraft:root / minecraft:trigger_type]: {trigger}"
            ));
        }
        let trigger = trigger.to_string();
        let conditions = criterion
            .get("conditions")
            .map_or_else(|| "{}".to_string(), ToString::to_string);
        let parsed = Arc::new(CriterionConditions::parse(&trigger, &conditions));
        criteria.push((name.clone(), trigger, parsed));
    }

    let names: Vec<&str> = criteria.iter().map(|(name, ..)| name.as_str()).collect();
    let requirements = parse_requirements(json, &names)?;

    let display = json.get("display").map(parse_display).transpose()?;
    if let Some(display) = &display {
        let has_background = display.background_texture.is_some();
        if parent.is_none() && !has_background {
            return Err("Visible advancement roots must have background".to_string());
        }
        if parent.is_some() && has_background {
            return Err("Only advancement roots can have background".to_string());
        }
    }

    let Rewards {
        experience,
        recipes,
        loot,
        function,
    } = parse_rewards(json)?;
    let send_telemetry = get_bool(json, "sends_telemetry_event", false)?;

    let key = format!("{id}\n{json}");
    let mut interned = INTERNED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let advancement = *interned.entry(key).or_insert_with(|| {
        let criteria_definitions: Vec<AdvancementCriterion> = criteria
            .iter()
            .map(|(name, trigger, _)| AdvancementCriterion {
                name: leak_str(name),
                trigger: leak_str(trigger),
                // Datapack criteria are matched with `AdvancementEntry::conditions`.
                conditions: AdvancementCondition::Empty,
            })
            .collect();
        let criteria_names: Vec<&'static str> =
            criteria_definitions.iter().map(|c| c.name).collect();
        let requirements: Vec<&'static [&'static str]> = requirements
            .iter()
            .map(|set| -> &'static [&'static str] {
                set.iter()
                    .map(|name| leak_str(name))
                    .collect::<Vec<_>>()
                    .leak()
            })
            .collect();
        let recipes: Vec<&'static str> = recipes.iter().map(|recipe| leak_str(recipe)).collect();
        Box::leak(Box::new(Advancement {
            id: id.clone(),
            parent: parent.clone(),
            send_telemetry,
            display: display.map(|display| &*Box::leak(Box::new(display))),
            reward: Box::leak(Box::new(AdvancementReward {
                experience,
                recipes: recipes.leak(),
            })),
            requirements: requirements.leak(),
            criteria: criteria_names.leak(),
            criteria_definitions: criteria_definitions.leak(),
        }))
    });
    drop(interned);

    Ok(AdvancementEntry {
        advancement,
        conditions: criteria
            .into_iter()
            .map(|(_, _, conditions)| conditions)
            .collect(),
        loot: loot.into(),
        function,
    })
}

/// `AdvancementRequirements.allOf` when absent, then `AdvancementRequirements.validate`.
fn parse_requirements(json: &Value, criteria: &[&str]) -> Result<Vec<Vec<String>>, String> {
    let requirements: Vec<Vec<String>> = match json.get("requirements") {
        Some(requirements) => serde_json::from_value(requirements.clone())
            .map_err(|_| "requirements must be lists of criterion names".to_string())?,
        None => criteria
            .iter()
            .map(|name| vec![(*name).to_string()])
            .collect(),
    };
    let mut missing: Vec<&str> = criteria
        .iter()
        .copied()
        .filter(|name| !requirements.iter().flatten().any(|r| r == name))
        .collect();
    let mut unknown: Vec<&str> = requirements
        .iter()
        .flatten()
        .map(String::as_str)
        .filter(|name| !criteria.contains(name))
        .collect();
    if !missing.is_empty() || !unknown.is_empty() {
        missing.sort_unstable();
        missing.dedup();
        unknown.sort_unstable();
        unknown.dedup();
        return Err(format!(
            "Advancement completion requirements did not exactly match specified criteria. Missing: [{}]. Unknown: [{}]",
            missing.join(", "),
            unknown.join(", ")
        ));
    }
    Ok(requirements)
}

/// Vanilla `AdvancementRewards`.
struct Rewards {
    experience: i32,
    recipes: Vec<String>,
    loot: Vec<String>,
    function: Option<Arc<str>>,
}

/// Vanilla `AdvancementRewards.CODEC`.
fn parse_rewards(json: &Value) -> Result<Rewards, String> {
    let rewards = json.get("rewards");
    let field = |key: &str| rewards.and_then(|rewards| rewards.get(key));
    let experience = field("experience")
        .map(|experience| {
            experience
                .as_i64()
                .and_then(|experience| i32::try_from(experience).ok())
                .ok_or("experience is not an integer")
        })
        .transpose()?
        .unwrap_or(0);
    let recipes = field("recipes")
        .and_then(Value::as_array)
        .map(|recipes| {
            recipes
                .iter()
                .map(|recipe| parse_identifier(recipe, "recipe").map(|id| id.to_string()))
                .collect::<Result<_, _>>()
        })
        .transpose()?
        .unwrap_or_default();
    let loot = match field("loot") {
        None => Vec::new(),
        Some(Value::Array(tables)) => tables
            .iter()
            .map(|table| parse_identifier(table, "loot table").map(|id| id.to_string()))
            .collect::<Result<_, _>>()?,
        Some(table) => vec![parse_identifier(table, "loot table")?.to_string()],
    };
    let function = field("function")
        .map(|function| parse_identifier(function, "function"))
        .transpose()?
        .map(|function| Arc::from(function.to_string()));
    Ok(Rewards {
        experience,
        recipes,
        loot,
        function,
    })
}

/// Vanilla `DisplayInfo.CODEC`. The icon's components aren't read.
fn parse_display(json: &Value) -> Result<AdvancementDisplay, String> {
    let icon = json.get("icon").ok_or("display has no icon")?;
    let (item_id, count) = match icon {
        Value::String(id) => (id.as_str(), 1),
        icon => {
            let id = icon
                .get("id")
                .and_then(Value::as_str)
                .ok_or("icon has no id")?;
            let count = icon.get("count").map_or(Ok(1), |count| {
                count
                    .as_u64()
                    .filter(|count| (1..=99).contains(count))
                    .ok_or_else(|| format!("invalid icon count {count}"))
            })?;
            (id, count)
        }
    };
    let item = Item::from_registry_key(item_id).ok_or_else(|| format!("unknown item {item_id}"))?;
    let text = |key: &str| -> Result<TextComponent, String> {
        let value = json
            .get(key)
            .ok_or_else(|| format!("display has no {key}"))?;
        serde_json::from_value(value.clone()).map_err(|_| format!("invalid {key}"))
    };
    let frame_type = json.get("frame").map_or(Ok(FrameType::Task), |frame| {
        serde_json::from_value(frame.clone()).map_err(|_| format!("invalid frame {frame}"))
    })?;
    let background = json
        .get("background")
        .map(|background| parse_identifier(background, "background"))
        .transpose()?;
    let mut display = AdvancementDisplay::new(
        "",
        "",
        ItemStack::new(count as u8, item),
        frame_type,
        background.map(|background| leak_str(&background.to_string())),
        get_bool(json, "show_toast", true)?,
        get_bool(json, "hidden", false)?,
        get_bool(json, "announce_to_chat", true)?,
        0.0,
        0.0,
    );
    display.title_component = Some(text("title")?);
    display.description_component = Some(text("description")?);
    Ok(display)
}

/// Vanilla `TreeNodePosition`: the Reingold-Tilford layout of one advancement tab.
mod tree_node_position {
    use super::ServerAdvancements;

    struct Position {
        node: usize,
        parent: Option<usize>,
        previous_sibling: Option<usize>,
        child_index: usize,
        children: Vec<usize>,
        ancestor: usize,
        thread: Option<usize>,
        x: i32,
        y: f32,
        modifier: f32,
        change: f32,
        shift: f32,
    }

    pub(super) fn run(tree: &mut ServerAdvancements, root: usize) {
        let mut positions = Vec::new();
        create(tree, &mut positions, root, None, None, 1, 0);
        first_walk(&mut positions, 0);
        let root_y = positions[0].y;
        let min = second_walk(&mut positions, 0, 0.0, 0, root_y);
        if min < 0.0 {
            // `thirdWalk` shifts every node of this tab.
            for position in &mut positions {
                position.y -= min;
            }
        }
        for position in &positions {
            let node = &mut tree.nodes[position.node];
            node.x = position.x as f32;
            node.y = position.y;
        }
    }

    /// The `TreeNodePosition` constructor.
    fn create(
        tree: &ServerAdvancements,
        positions: &mut Vec<Position>,
        node: usize,
        parent: Option<usize>,
        previous_sibling: Option<usize>,
        child_index: usize,
        depth: i32,
    ) -> usize {
        let index = positions.len();
        positions.push(Position {
            node,
            parent,
            previous_sibling,
            child_index,
            children: Vec::new(),
            ancestor: index,
            thread: None,
            x: depth,
            y: -1.0,
            modifier: 0.0,
            change: 0.0,
            shift: 0.0,
        });
        let mut previous = None;
        for &child in &tree.node(node).children {
            previous = add_child(tree, positions, index, child, previous);
        }
        index
    }

    /// `TreeNodePosition.addChild`: advancements without a display are skipped, their
    /// children placed in their stead.
    fn add_child(
        tree: &ServerAdvancements,
        positions: &mut Vec<Position>,
        parent: usize,
        node: usize,
        mut previous: Option<usize>,
    ) -> Option<usize> {
        if tree.node_advancement(node).display.is_some() {
            let child_index = positions[parent].children.len() + 1;
            let depth = positions[parent].x + 1;
            let child = create(
                tree,
                positions,
                node,
                Some(parent),
                previous,
                child_index,
                depth,
            );
            positions[parent].children.push(child);
            Some(child)
        } else {
            for &grandchild in &tree.node(node).children {
                previous = add_child(tree, positions, parent, grandchild, previous);
            }
            previous
        }
    }

    fn first_walk(positions: &mut [Position], index: usize) {
        if positions[index].children.is_empty() {
            positions[index].y = positions[index]
                .previous_sibling
                .map_or(0.0, |sibling| positions[sibling].y + 1.0);
            return;
        }
        let mut default_ancestor = None;
        for i in 0..positions[index].children.len() {
            let child = positions[index].children[i];
            first_walk(positions, child);
            default_ancestor = Some(apportion(
                positions,
                child,
                default_ancestor.unwrap_or(child),
            ));
        }
        execute_shifts(positions, index);
        let children = &positions[index].children;
        #[expect(clippy::manual_midpoint, reason = "vanilla's float arithmetic")]
        let midpoint = (positions[children[0]].y + positions[children[children.len() - 1]].y) / 2.0;
        if let Some(sibling) = positions[index].previous_sibling {
            positions[index].y = positions[sibling].y + 1.0;
            positions[index].modifier = positions[index].y - midpoint;
        } else {
            positions[index].y = midpoint;
        }
    }

    fn second_walk(
        positions: &mut [Position],
        index: usize,
        modifier_sum: f32,
        depth: i32,
        mut min: f32,
    ) -> f32 {
        positions[index].y += modifier_sum;
        positions[index].x = depth;
        if positions[index].y < min {
            min = positions[index].y;
        }
        let modifier = positions[index].modifier;
        for i in 0..positions[index].children.len() {
            let child = positions[index].children[i];
            min = second_walk(positions, child, modifier_sum + modifier, depth + 1, min);
        }
        min
    }

    fn execute_shifts(positions: &mut [Position], index: usize) {
        let mut shift = 0.0;
        let mut change = 0.0;
        for i in (0..positions[index].children.len()).rev() {
            let child = &mut positions[positions[index].children[i]];
            child.y += shift;
            child.modifier += shift;
            change += child.change;
            shift += child.shift + change;
        }
    }

    fn previous_or_thread(positions: &[Position], index: usize) -> Option<usize> {
        positions[index]
            .thread
            .or_else(|| positions[index].children.first().copied())
    }

    fn next_or_thread(positions: &[Position], index: usize) -> Option<usize> {
        positions[index]
            .thread
            .or_else(|| positions[index].children.last().copied())
    }

    fn apportion(positions: &mut [Position], index: usize, mut default_ancestor: usize) -> usize {
        let Some(previous_sibling) = positions[index].previous_sibling else {
            return default_ancestor;
        };
        let Some(parent) = positions[index].parent else {
            return default_ancestor;
        };
        let mut vir = index;
        let mut vor = index;
        let mut vil = previous_sibling;
        let mut vol = positions[parent].children[0];
        let mut sir = positions[index].modifier;
        let mut sor = positions[index].modifier;
        let mut sil = positions[vil].modifier;
        let mut sol = positions[vol].modifier;
        while let (Some(next_vil), Some(next_vir)) = (
            next_or_thread(positions, vil),
            previous_or_thread(positions, vir),
        ) {
            vil = next_vil;
            vir = next_vir;
            // Both outer contours are at least as deep as the inner ones.
            let (Some(next_vol), Some(next_vor)) = (
                previous_or_thread(positions, vol),
                next_or_thread(positions, vor),
            ) else {
                break;
            };
            vol = next_vol;
            vor = next_vor;
            positions[vor].ancestor = index;
            let shift = positions[vil].y + sil - (positions[vir].y + sir) + 1.0;
            if shift > 0.0 {
                let ancestor = get_ancestor(positions, vil, index, default_ancestor);
                move_subtree(positions, ancestor, index, shift);
                sir += shift;
                sor += shift;
            }
            sil += positions[vil].modifier;
            sir += positions[vir].modifier;
            sol += positions[vol].modifier;
            sor += positions[vor].modifier;
        }
        if let Some(next_vil) = next_or_thread(positions, vil)
            && next_or_thread(positions, vor).is_none()
        {
            positions[vor].thread = Some(next_vil);
            positions[vor].modifier += sil - sor;
        } else {
            if let Some(next_vir) = previous_or_thread(positions, vir)
                && previous_or_thread(positions, vol).is_none()
            {
                positions[vol].thread = Some(next_vir);
                positions[vol].modifier += sir - sol;
            }
            default_ancestor = index;
        }
        default_ancestor
    }

    fn move_subtree(positions: &mut [Position], left: usize, right: usize, shift: f32) {
        let subtrees = positions[right].child_index as f32 - positions[left].child_index as f32;
        if subtrees != 0.0 {
            positions[right].change -= shift / subtrees;
            positions[left].change += shift / subtrees;
        }
        positions[right].shift += shift;
        positions[right].y += shift;
        positions[right].modifier += shift;
    }

    fn get_ancestor(
        positions: &[Position],
        index: usize,
        other: usize,
        default_ancestor: usize,
    ) -> usize {
        let ancestor = positions[index].ancestor;
        let in_siblings = positions[other]
            .parent
            .is_some_and(|parent| positions[parent].children.contains(&ancestor));
        if in_siblings {
            ancestor
        } else {
            default_ancestor
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The layout reproduces the positions codegen computed for the built-in tabs.
    #[test]
    fn built_in_layout_matches_generated() {
        let advancements = ServerAdvancements::default();
        for node in &advancements.nodes {
            let advancement = advancements.entries[node.entry].advancement;
            if let Some(display) = advancement.display {
                assert_eq!(
                    (node.x, node.y),
                    (display.x, display.y),
                    "{}",
                    advancement.id
                );
            }
        }
    }

    #[test]
    fn datapack_advancements_join_the_tree() {
        let mut datapack = HashMap::new();
        datapack.insert(
            "p:root".to_string(),
            serde_json::json!({
                "display": {"icon": "minecraft:stick", "title": "Root", "description": "d",
                    "background": "minecraft:block/stone"},
                "criteria": {"tick": {"trigger": "minecraft:tick"}}
            }),
        );
        datapack.insert(
            "p:child".to_string(),
            serde_json::json!({
                "parent": "p:root",
                "display": {"icon": {"id": "minecraft:stone", "count": 2}, "title": "C", "description": "d"},
                "criteria": {"a": {"trigger": "minecraft:impossible"}, "b": {"trigger": "minecraft:impossible"}},
                "requirements": [["a", "b"]],
                "rewards": {"function": "p:reward", "loot": "p:loot"}
            }),
        );
        datapack.insert(
            "p:orphan".to_string(),
            serde_json::json!({"parent": "p:nope", "criteria": {"a": {"trigger": "minecraft:impossible"}}}),
        );
        datapack.insert(
            "p:bad_requirements".to_string(),
            serde_json::json!({"criteria": {"a": {"trigger": "minecraft:impossible"}}, "requirements": [["b"]]}),
        );
        datapack.insert(
            "minecraft:story/root".to_string(),
            serde_json::json!({"criteria": {"x": {"trigger": "minecraft:impossible"}}}),
        );
        datapack.insert(
            "p:bad_trigger".to_string(),
            serde_json::json!({"criteria": {"a": {"trigger": "p:nope"}}}),
        );
        let mut errors = RegistryLoadErrors::default();
        let advancements = ServerAdvancements::new(&datapack, &mut errors);
        assert_eq!(
            errors.get(ADVANCEMENT_REGISTRY, "p:bad_trigger"),
            Some(
                "Unknown registry key in ResourceKey[minecraft:root / minecraft:trigger_type]: p:nope"
            )
        );
        assert!(
            errors
                .get(ADVANCEMENT_REGISTRY, "p:bad_requirements")
                .is_some()
        );

        let child = advancements.get_by_name("p:child").expect("child loads");
        assert_eq!(child.requirements, &[&["a", "b"][..]]);
        let child_node = advancements
            .node_of(&child.id)
            .expect("child is in the tree");
        let root_node = advancements.root(child_node);
        assert_eq!(
            advancements.node_advancement(root_node).id.to_string(),
            "p:root"
        );
        assert_eq!(
            (
                advancements.node(child_node).x,
                advancements.node(child_node).y
            ),
            (1.0, 0.0)
        );
        let entry = advancements.get(&child.id).expect("entry");
        assert_eq!(entry.function.as_deref(), Some("p:reward"));
        assert_eq!(&*entry.loot, &["p:loot".to_string()]);

        // Out of the tree, but still an advancement like in vanilla.
        let orphan = advancements.get_by_name("p:orphan").expect("orphan loads");
        assert!(advancements.node_of(&orphan.id).is_none());
        assert!(advancements.get_by_name("p:bad_requirements").is_none());

        // The override has no display, so its tab's children now hang off an invisible root.
        let story_root = advancements
            .get_by_name("minecraft:story/root")
            .expect("override loads");
        assert!(story_root.display.is_none());
        assert_eq!(story_root.criteria, &["x"]);
    }
}
