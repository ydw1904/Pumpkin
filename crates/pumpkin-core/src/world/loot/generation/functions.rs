//! The loot functions that work on an item's components, read from their JSON when applied.

use std::borrow::Cow;

use pumpkin_data::Enchantment;
use pumpkin_data::data_component::DataComponent;
use pumpkin_data::data_component_impl::{
    CustomDataImpl, DataComponentImpl, InstrumentImpl, LoreImpl, OminousBottleAmplifierImpl,
    PotionContentsImpl, SuspiciousStewEffect, SuspiciousStewEffectsImpl, read_data,
};
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::map_decoration::MapDecorationType;
use pumpkin_data::potion::Potion;
use pumpkin_data::tag::{RegistryKey, get_tag_values};
use pumpkin_nbt::tag::NbtTag;
use pumpkin_util::loot_table::parser::{float_provider, int_provider, parse_functions};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::text::TextComponent;
use serde_json::Value;

use super::super::predicates::{item_matches, json_to_nbt};
use super::{Functions, Generator, RandomImpl, Stack};

/// `ExplorationMapFunction.DEFAULT_ZOOM`.
const DEFAULT_ZOOM: i8 = 2;
/// `ExplorationMapFunction.DEFAULT_SEARCH_RADIUS`.
const DEFAULT_SEARCH_RADIUS: i32 = 50;
/// The most lines `ItemLore` holds.
const MAX_LORE_LINES: usize = 256;
/// The highest `ominous_bottle_amplifier`.
const MAX_OMINOUS_BOTTLE_AMPLIFIER: i32 = 4;

fn function_type(json: &Value) -> &str {
    let name = json
        .get("type")
        .or_else(|| json.get("function"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    name.strip_prefix("minecraft:").unwrap_or(name)
}

/// The entries of a holder set: a `#tag`, one id or a list of ids, namespaced.
fn holder_set(value: &Value, registry: RegistryKey) -> Vec<String> {
    let namespaced = |name: &str| {
        if name.contains(':') {
            name.to_owned()
        } else {
            format!("minecraft:{name}")
        }
    };
    match value {
        Value::String(name) => name.strip_prefix('#').map_or_else(
            || vec![namespaced(name)],
            |tag| {
                get_tag_values(registry, &namespaced(tag))
                    .unwrap_or_default()
                    .iter()
                    .map(|name| namespaced(name))
                    .collect()
            },
        ),
        Value::Array(names) => names
            .iter()
            .filter_map(Value::as_str)
            .map(namespaced)
            .collect(),
        _ => Vec::new(),
    }
}

fn text(value: &Value) -> Option<TextComponent> {
    json_to_nbt(value).map(|tag| TextComponent::from_nbt(&tag))
}

/// Sets a component the way `ItemStack.set` does, or removes it for `None`.
fn put_component(
    stack: &mut Stack,
    id: DataComponent,
    component: Option<Box<dyn DataComponentImpl>>,
) {
    put_item_component(&mut stack.stack, id, component);
}

fn put_item_component(
    stack: &mut ItemStack,
    id: DataComponent,
    component: Option<Box<dyn DataComponentImpl>>,
) {
    let Some(component) = component else {
        stack.remove_data_component(id);
        return;
    };
    if let Some((_, existing)) = stack.patch.iter_mut().find(|(seen, _)| *seen == id) {
        *existing = Some(component);
    } else {
        stack.patch.push((id, Some(component)));
    }
}

/// Applies a `DataComponentPatch` written as JSON, where `!name` removes a component. Values
/// Pumpkin can't read are left out; an unknown component name is returned as the error after
/// the rest of the patch is applied.
pub fn apply_component_patch(
    stack: &mut ItemStack,
    components: &serde_json::Map<String, Value>,
) -> Result<(), String> {
    let mut unknown = None;
    for (name, value) in components {
        let (removed, component_name) = name
            .strip_prefix('!')
            .map_or((false, name.as_str()), |name| (true, name));
        let Some(id) = DataComponent::try_from_name(component_name) else {
            unknown.get_or_insert_with(|| format!("unknown component {name}"));
            continue;
        };
        if removed {
            put_item_component(stack, id, None);
        } else if let Some(component) = json_to_nbt(value).and_then(|tag| read_data(id, &tag)) {
            put_item_component(stack, id, Some(component));
        }
    }
    unknown.map_or(Ok(()), Err)
}

impl Generator<'_, '_> {
    /// `LootItemFunction.apply` for the functions kept as JSON.
    pub(super) fn apply_json(&mut self, json: &Value, stack: &mut Stack) {
        match function_type(json) {
            "discard" => stack.clear(),
            "sequence" => self.apply_nested(json.get("functions"), stack),
            "filtered" => {
                // Item predicates can test the count, which the stack only gets when split.
                stack.stack.item_count = stack.count.clamp(0, i32::from(u8::MAX)) as u8;
                let passed = json
                    .get("item_filter")
                    .is_some_and(|filter| item_matches(&stack.stack, filter));
                let branch = if passed { "on_pass" } else { "on_fail" };
                self.apply_nested(json.get(branch), stack);
            }
            "set_item" => {
                if let Some(item) = json
                    .get("item")
                    .and_then(Value::as_str)
                    .and_then(|name| Item::from_registry_key(name.trim_start_matches("minecraft:")))
                {
                    // `transmuteCopy` keeps the components and the count.
                    stack.stack.item = item;
                }
            }
            "set_components" => Self::set_components(json, stack),
            "set_name" => Self::set_name(json, stack),
            "set_lore" => Self::set_lore(json, stack),
            "set_custom_data" => Self::set_custom_data(json, stack),
            "set_damage" => self.set_damage(json, stack),
            "set_potion" => {
                if let Some(potion) = json.get("id").and_then(Value::as_str) {
                    set_potion(stack, potion);
                }
            }
            "set_stew_effect" => self.set_stew_effect(json, stack),
            "set_ominous_bottle_amplifier" => {
                if let Some(amplifier) = json.get("amplifier").and_then(int_provider) {
                    let amplifier = self
                        .number_int(amplifier)
                        .clamp(0, MAX_OMINOUS_BOTTLE_AMPLIFIER);
                    stack
                        .stack
                        .set_data_component(OminousBottleAmplifierImpl { amplifier });
                }
            }
            "set_instrument" => {
                let options = json.get("options").map_or_else(Vec::new, |options| {
                    holder_set(options, RegistryKey::Instrument)
                });
                if let Some(instrument) = self.random_element(&options) {
                    stack.stack.set_data_component(InstrumentImpl {
                        instrument: Cow::Owned(instrument.clone()),
                    });
                }
            }
            "enchant_randomly" => self.enchant_randomly(json, stack),
            "enchant_with_levels" => self.enchant_with_levels(json, stack),
            "set_enchantments" => self.set_enchantments(json, stack),
            "exploration_map" => self.exploration_map(json, stack),
            _ => {}
        }
    }

    /// The function or list of functions under `value`, applied in order.
    fn apply_nested(&mut self, value: Option<&Value>, stack: &mut Stack) {
        if let Some(value) = value {
            let functions = parse_functions(value);
            self.apply(stack, &[Functions::Dynamic(&functions)]);
        }
    }

    /// `Util.getRandomSafe`.
    fn random_element<'a, T>(&mut self, options: &'a [T]) -> Option<&'a T> {
        if options.is_empty() {
            return None;
        }
        options.get(self.rng.next_bounded_i32(options.len() as i32) as usize)
    }

    /// `SetComponentsFunction`: applies a component patch; `!name` removes a component.
    fn set_components(json: &Value, stack: &mut Stack) {
        if let Some(components) = json.get("components").and_then(Value::as_object) {
            let _ = apply_component_patch(&mut stack.stack, components);
        }
    }

    /// `SetNameFunction`, without resolving score and selector components.
    fn set_name(json: &Value, stack: &mut Stack) {
        let Some(name) = json.get("name") else {
            return;
        };
        let id = match json.get("target").and_then(Value::as_str) {
            Some("item_name") => DataComponent::ItemName,
            _ => DataComponent::CustomName,
        };
        if let Some(component) = json_to_nbt(name).and_then(|tag| read_data(id, &tag)) {
            put_component(stack, id, Some(component));
        }
    }

    /// `SetLoreFunction` with its `ListOperation`, without resolving score and selector
    /// components.
    fn set_lore(json: &Value, stack: &mut Stack) {
        let lines: Vec<TextComponent> = json
            .get("lore")
            .and_then(Value::as_array)
            .map(|lines| lines.iter().filter_map(text).collect())
            .unwrap_or_default();
        let original = stack
            .stack
            .get_data_component::<LoreImpl>()
            .map(|lore| lore.lines.clone())
            .unwrap_or_default();
        let offset = json
            .get("offset")
            .and_then(Value::as_u64)
            .map_or(0, |offset| offset as usize);
        let lore = match json.get("mode").and_then(Value::as_str) {
            Some("replace_all") => lines,
            Some("insert") => {
                if offset > original.len() || original.len() + lines.len() > MAX_LORE_LINES {
                    original
                } else {
                    let mut lore = original[..offset].to_vec();
                    lore.extend(lines);
                    lore.extend_from_slice(&original[offset..]);
                    lore
                }
            }
            Some("replace_section") => {
                if offset > original.len() {
                    original
                } else {
                    let resume = offset
                        + json
                            .get("size")
                            .and_then(Value::as_u64)
                            .map_or(lines.len(), |size| size as usize);
                    let mut lore = original[..offset].to_vec();
                    lore.extend(lines);
                    if resume < original.len() {
                        lore.extend_from_slice(&original[resume..]);
                    }
                    if lore.len() > MAX_LORE_LINES {
                        original
                    } else {
                        lore
                    }
                }
            }
            _ => {
                if original.len() + lines.len() > MAX_LORE_LINES {
                    original
                } else {
                    let mut lore = original;
                    lore.extend(lines);
                    lore
                }
            }
        };
        stack.stack.set_data_component(LoreImpl { lines: lore });
    }

    /// `SetCustomDataFunction`: merges the tag into the item's custom data.
    fn set_custom_data(json: &Value, stack: &mut Stack) {
        let tag = match json.get("tag") {
            Some(Value::String(snbt)) => {
                let mut reader = crate::command::string_reader::StringReader::new(snbt.as_str());
                crate::command::snbt::SnbtParser::parse_for_commands(&mut reader).ok()
            }
            Some(value) => json_to_nbt(value),
            None => None,
        };
        let Some(NbtTag::Compound(tag)) = tag else {
            return;
        };
        let mut data = stack
            .stack
            .get_data_component::<CustomDataImpl>()
            .map(|data| data.data.clone())
            .unwrap_or_default();
        data.merge(&tag);
        stack.stack.set_data_component(CustomDataImpl { data });
    }

    /// `SetItemDamageFunction`: `damage` is the fraction of durability left.
    fn set_damage(&mut self, json: &Value, stack: &mut Stack) {
        let Some(damage) = json.get("damage").and_then(float_provider) else {
            return;
        };
        let Some(max_damage) = stack
            .stack
            .is_damageable()
            .then(|| stack.stack.get_max_damage())
            .flatten()
        else {
            return;
        };
        let add = json.get("add").and_then(Value::as_bool).unwrap_or(false);
        let base = if add {
            1.0 - stack.stack.get_damage() as f32 / max_damage as f32
        } else {
            0.0
        };
        let fraction = 1.0 - (self.number_float(damage) + base).clamp(0.0, 1.0);
        stack
            .stack
            .set_damage((fraction * max_damage as f32).floor() as i32);
    }

    /// `SetStewEffectFunction`: one random effect, in seconds unless it is instant.
    fn set_stew_effect(&mut self, json: &Value, stack: &mut Stack) {
        if stack.stack.item != &Item::SUSPICIOUS_STEW {
            return;
        }
        let effects: Vec<&Value> = json
            .get("effects")
            .and_then(Value::as_array)
            .map(|effects| effects.iter().collect())
            .unwrap_or_default();
        let Some(&entry) = self.random_element(&effects) else {
            return;
        };
        let Some(effect) = entry
            .get("type")
            .and_then(Value::as_str)
            .and_then(|name| StatusEffect::from_name(name.trim_start_matches("minecraft:")))
        else {
            return;
        };
        let Some(duration) = entry.get("duration").and_then(int_provider) else {
            return;
        };
        let mut duration = self.number_int(duration);
        if !is_instantaneous(effect) {
            duration *= 20;
        }
        let mut effects = stack
            .stack
            .get_data_component::<SuspiciousStewEffectsImpl>()
            .map(|effects| effects.effects.to_vec())
            .unwrap_or_default();
        effects.push(SuspiciousStewEffect {
            effect: Cow::Owned(format!("minecraft:{}", effect.minecraft_name)),
            duration,
        });
        stack.stack.set_data_component(SuspiciousStewEffectsImpl {
            effects: Cow::Owned(effects),
        });
    }

    /// The `options` of an enchanting function, or every enchantment.
    fn enchantment_options(json: &Value) -> Vec<&'static Enchantment> {
        json.get("options").map_or_else(
            || Enchantment::all().copied().collect(),
            |options| {
                holder_set(options, RegistryKey::Enchantment)
                    .iter()
                    .filter_map(|name| Enchantment::from_name(name))
                    .collect()
            },
        )
    }

    /// `EnchantRandomlyFunction`: one random enchantment at a random level; a book becomes an
    /// enchanted book.
    fn enchant_randomly(&mut self, json: &Value, stack: &mut Stack) {
        let is_book = stack.stack.item == &Item::BOOK;
        let only_compatible = json
            .get("only_compatible")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let check_compatibility = !is_book && only_compatible;
        let options: Vec<_> = Self::enchantment_options(json)
            .into_iter()
            .filter(|enchantment| !check_compatibility || enchantment.can_enchant(stack.stack.item))
            .collect();
        let Some(&enchantment) = self.random_element(&options) else {
            tracing::warn!(
                "Couldn't find a compatible enchantment for {}",
                stack.stack.item.registry_key
            );
            return;
        };
        let level = if enchantment.max_level <= 1 {
            1
        } else {
            1 + self.rng.next_bounded_i32(enchantment.max_level)
        };
        if is_book {
            stack.stack = pumpkin_data::item_stack::ItemStack::new(1, &Item::ENCHANTED_BOOK);
            stack.count = 1;
        }
        stack.stack.enchant(enchantment, level);
    }

    /// `EnchantWithLevelsFunction` and `EnchantmentHelper.enchantItem`.
    fn enchant_with_levels(&mut self, json: &Value, stack: &mut Stack) {
        let Some(levels) = json.get("levels").and_then(int_provider) else {
            return;
        };
        let cost = self.number_int(levels);
        let options = Self::enchantment_options(json);
        let enchantments = pumpkin_inventory::enchanting::select_enchantment(
            &mut self.rng,
            &stack.stack,
            cost,
            options.into_iter(),
        );
        if stack.stack.item == &Item::BOOK {
            stack.stack = pumpkin_data::item_stack::ItemStack::new(1, &Item::ENCHANTED_BOOK);
            stack.count = 1;
        }
        for (enchantment, level) in enchantments {
            stack.stack.enchant(enchantment, level);
        }
    }

    /// `SetEnchantmentsFunction`: sets or adds levels; a book becomes an enchanted book.
    fn set_enchantments(&mut self, json: &Value, stack: &mut Stack) {
        if stack.stack.item == &Item::BOOK {
            stack.stack.item = &Item::ENCHANTED_BOOK;
        }
        let add = json.get("add").and_then(Value::as_bool).unwrap_or(false);
        let Some(levels) = json.get("enchantments").and_then(Value::as_object) else {
            return;
        };
        let mut enchantments =
            pumpkin_inventory::anvil::anvil_screen_handler::get_enchantments_for_crafting(
                &stack.stack,
            );
        for (name, level) in levels {
            let (Some(enchantment), Some(level)) =
                (Enchantment::from_name(name), int_provider(level))
            else {
                continue;
            };
            let current = enchantments
                .iter()
                .find(|(seen, _)| *seen == enchantment)
                .map_or(0, |(_, level)| *level);
            let level = self.number_int(level);
            let level = if add { current + level } else { level }.clamp(0, 255);
            // `ItemEnchantments.Mutable.set`: a level of 0 removes the enchantment.
            match enchantments
                .iter()
                .position(|(seen, _)| *seen == enchantment)
            {
                Some(index) if level == 0 => {
                    enchantments.remove(index);
                }
                Some(index) => enchantments[index].1 = level,
                None if level > 0 => enchantments.push((enchantment, level)),
                None => {}
            }
        }
        pumpkin_inventory::anvil::anvil_screen_handler::set_enchantments(
            &mut stack.stack,
            enchantments,
        );
    }

    /// `ExplorationMapFunction`: turns the map into an exploration map of the nearest structure
    /// of the destination. With `skip_existing_chunks`, each map claims its structure so later
    /// maps lead elsewhere. Vanilla's `claimStructureAt`, which first claims a destination
    /// structure the loot origin is inside, isn't ported: chunk structure references don't
    /// record their start chunk.
    fn exploration_map(&self, json: &Value, stack: &mut Stack) {
        if stack.is_empty() {
            return;
        }
        let (Some(world), Some(origin)) = (self.params.world, self.params.position) else {
            return;
        };
        let Some(destination) = json.get("destination") else {
            return;
        };
        let structures = holder_set(destination, RegistryKey::WorldgenStructure);
        let decoration = json
            .get("decoration")
            .and_then(Value::as_str)
            .map_or(Some(&MapDecorationType::MANSION), |name| {
                MapDecorationType::from_name(name.trim_start_matches("minecraft:"))
            });
        let Some(decoration) = decoration else {
            return;
        };
        let zoom = json
            .get("zoom")
            .and_then(Value::as_i64)
            .map_or(DEFAULT_ZOOM, |zoom| zoom as i8);
        let search_radius = json
            .get("search_radius")
            .and_then(Value::as_i64)
            .map_or(DEFAULT_SEARCH_RADIUS, |radius| radius as i32);
        let skip_known_structures = json
            .get("skip_existing_chunks")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let origin = BlockPos::containing_vec(origin);
        let Some(target) = world.find_nearest_map_structure(
            &structures,
            origin,
            search_radius,
            skip_known_structures,
        ) else {
            return;
        };
        crate::world::map::make_exploration_map(world, &mut stack.stack, target, zoom, decoration);
    }
}

/// `SetPotionFunction`.
fn set_potion(stack: &mut Stack, name: &str) {
    let Some(potion) = Potion::from_name(name.trim_start_matches("minecraft:")) else {
        return;
    };
    let mut contents = stack
        .stack
        .get_data_component::<PotionContentsImpl>()
        .cloned()
        .unwrap_or(PotionContentsImpl {
            potion_id: None,
            custom_color: None,
            custom_effects: Vec::new(),
            custom_name: None,
        });
    contents.potion_id = Some(i32::from(potion.id));
    stack.stack.set_data_component(contents);
}

/// `MobEffect.isInstantaneous`: the effects that are `InstantenousMobEffect`s.
fn is_instantaneous(effect: &StatusEffect) -> bool {
    [
        &StatusEffect::INSTANT_HEALTH,
        &StatusEffect::INSTANT_DAMAGE,
        &StatusEffect::SATURATION,
    ]
    .contains(&effect)
}
