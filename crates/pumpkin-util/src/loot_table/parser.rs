use std::cell::Cell;

use serde_json::Value;

use super::number::{
    FloatProvider, IntProvider, LevelBased, ScoreEntity, ScoreTarget, StoredNumber,
};
use super::{
    DynamicLootCondition as Condition, DynamicLootEntry as Entry,
    DynamicLootEntryKind as EntryKind, DynamicLootFunction as Function, DynamicLootPool as Pool,
    DynamicLootTable as Table, JSON_LOOT_FUNCTIONS, LootBonusFormula, LootEntityTarget,
    LootFunctionKind, LootNumberProvider, SlotSource, SlotSourceTarget,
};

const MAX_PREDICATE_DEPTH: usize = 128;

/// Looks up the elements of the registries a loot table can refer to by id, as JSON. Each
/// lookup gets a namespaced id.
pub trait LootResolver {
    fn predicate(&self, _id: &str) -> Option<Value> {
        None
    }
    fn item_modifier(&self, _id: &str) -> Option<Value> {
        None
    }
    fn int_provider(&self, _id: &str) -> Option<Value> {
        None
    }
    fn float_provider(&self, _id: &str) -> Option<Value> {
        None
    }
    fn slot_source(&self, _id: &str) -> Option<Value> {
        None
    }
}

/// Resolves only predicates, with a function.
struct PredicateResolver<'a, F>(&'a F);

impl<F: Fn(&str) -> Option<Value>> LootResolver for PredicateResolver<'_, F> {
    fn predicate(&self, id: &str) -> Option<Value> {
        (self.0)(id)
    }
}

/// Resolves nothing.
struct NoResolver;

impl LootResolver for NoResolver {}

/// Parses loot tables while preserving composite entries and nested table references.
#[must_use]
pub fn parse_loot_table(json: &str) -> Option<Table> {
    parse_loot_table_with(json, &NoResolver)
}

/// Parses a condition with a resolver for named predicate definitions.
#[must_use]
pub fn parse_condition_with_predicates(
    value: &Value,
    predicates: &impl Fn(&str) -> Option<Value>,
) -> Option<Condition> {
    Parser::new(&PredicateResolver(predicates)).condition(value, 0)
}

/// Parses a condition with the registries it can refer to.
#[must_use]
pub fn parse_condition_with(value: &Value, resolver: &dyn LootResolver) -> Option<Condition> {
    Parser::new(resolver).condition(value, 0)
}

/// Parses a condition without predicate resolution.
#[must_use]
pub fn parse_condition(value: &Value) -> Option<Condition> {
    parse_condition_with(value, &NoResolver)
}

/// Parses a function, or a list of them, like the `on_pass` of a `filtered` function.
#[must_use]
pub fn parse_functions(value: &Value) -> Vec<Function> {
    parse_functions_with(value, &NoResolver)
}

/// Parses a function, a list of them or an item modifier id, with the registries they can
/// refer to.
#[must_use]
pub fn parse_functions_with(value: &Value, resolver: &dyn LootResolver) -> Vec<Function> {
    Parser::new(resolver).function_list(value)
}

/// Parses a loot table with a resolver for named predicate definitions.
pub fn parse_loot_table_with_predicates(
    json: &str,
    predicates: &impl Fn(&str) -> Option<Value>,
) -> Option<Table> {
    parse_loot_table_with(json, &PredicateResolver(predicates))
}

/// Parses a loot table with the registries it can refer to.
pub fn parse_loot_table_with(json: &str, resolver: &dyn LootResolver) -> Option<Table> {
    Parser::new(resolver).table(&serde_json::from_str::<Value>(json).ok()?)
}

/// Parses a number provider that is read as an int, without resolving references.
#[must_use]
pub fn int_provider(value: &Value) -> Option<LootNumberProvider> {
    Parser::new(&NoResolver).int_number(value)
}

/// Parses a number provider that is read as a float, without resolving references.
#[must_use]
pub fn float_provider(value: &Value) -> Option<LootNumberProvider> {
    Parser::new(&NoResolver).float_number(value)
}

struct Parser<'a> {
    resolver: &'a dyn LootResolver,
    /// How many references the element being parsed went through, to stop reference cycles.
    depth: Cell<usize>,
}

fn kind(value: &Value) -> &str {
    let name = value
        .get("type")
        .or_else(|| value.get("condition"))
        .or_else(|| value.get("function"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    name.strip_prefix("minecraft:").unwrap_or(name)
}

fn many(value: &Value) -> &[Value] {
    value
        .as_array()
        .map_or_else(|| std::slice::from_ref(value), Vec::as_slice)
}

fn all(conditions: Vec<Condition>) -> Condition {
    match conditions.len() {
        0 => Condition::None,
        1 => conditions.into_iter().next().unwrap_or_default(),
        _ => Condition::AllOf(conditions),
    }
}

fn number(value: &Value) -> Option<i32> {
    value.as_f64().map(|number| number as i32)
}

fn scalar(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.get("value").and_then(Value::as_f64))
}

fn namespaced(id: &str) -> String {
    if id.contains(':') {
        id.to_owned()
    } else {
        format!("minecraft:{id}")
    }
}

/// `LevelBasedValue.CODEC`.
fn level_based(value: &Value) -> Option<LevelBased> {
    if let Some(constant) = value.as_f64() {
        return Some(LevelBased::Constant(constant as f32));
    }
    let float = |key: &str| value.get(key)?.as_f64().map(|value| value as f32);
    let nested = |key: &str| level_based(value.get(key)?).map(Box::new);
    Some(match kind(value) {
        "linear" => LevelBased::Linear {
            base: float("base")?,
            per_level_above_first: float("per_level_above_first")?,
        },
        "clamped" => {
            let (min, max) = (float("min")?, float("max")?);
            if max <= min {
                return None;
            }
            LevelBased::Clamped {
                value: nested("value")?,
                min,
                max,
            }
        }
        "fraction" => LevelBased::Fraction {
            numerator: nested("numerator")?,
            denominator: nested("denominator")?,
        },
        "levels_squared" => LevelBased::LevelsSquared {
            added: float("added")?,
        },
        "exponent" => LevelBased::Exponent {
            base: nested("base")?,
            power: nested("power")?,
        },
        "lookup" => LevelBased::Lookup {
            values: value
                .get("values")?
                .as_array()?
                .iter()
                .map(|value| value.as_f64().map(|value| value as f32))
                .collect::<Option<_>>()?,
            fallback: nested("fallback")?,
        },
        _ => return None,
    })
}

/// `LootContext.EntityTarget.CODEC`.
fn score_entity(name: &str) -> Option<ScoreEntity> {
    Some(match name {
        "this" => ScoreEntity::Target(LootEntityTarget::This),
        "attacker" => ScoreEntity::Target(LootEntityTarget::Attacker),
        "direct_attacker" => ScoreEntity::Target(LootEntityTarget::DirectAttacker),
        "attacking_player" => ScoreEntity::AttackingPlayer,
        "target_entity" => ScoreEntity::TargetEntity,
        "interacting_entity" => ScoreEntity::InteractingEntity,
        _ => return None,
    })
}

/// `ScoreboardNameProviders.CODEC`: an entity target name, or a `fixed` or `context` provider.
fn score_target(value: &Value) -> Option<ScoreTarget> {
    if let Some(name) = value.as_str() {
        return score_entity(name).map(ScoreTarget::Entity);
    }
    match kind(value) {
        "fixed" => Some(ScoreTarget::Fixed(value.get("name")?.as_str()?.to_owned())),
        "context" => score_entity(value.get("target")?.as_str()?).map(ScoreTarget::Entity),
        _ => None,
    }
}

/// `StoredNumberAccess.MAP_CODEC`.
fn stored_number(value: &Value) -> Option<StoredNumber> {
    Some(StoredNumber {
        storage: namespaced(value.get("storage")?.as_str()?),
        path: value.get("path")?.as_str()?.to_owned(),
    })
}

fn environment_attribute(value: &Value) -> Option<String> {
    let name = value.get("attribute")?.as_str()?;
    Some(name.strip_prefix("minecraft:").unwrap_or(name).to_owned())
}

/// The plain [`LootNumberProvider`] form of an int provider, when it has one.
fn simple_int(provider: &IntProvider) -> Option<LootNumberProvider> {
    Some(match provider {
        IntProvider::Constant(value) => LootNumberProvider::Constant(f64::from(*value)),
        IntProvider::Uniform { min, max } => match (&**min, &**max) {
            (IntProvider::Constant(min), IntProvider::Constant(max)) => {
                LootNumberProvider::Uniform {
                    min: f64::from(*min),
                    max: f64::from(*max),
                }
            }
            _ => return None,
        },
        IntProvider::Binomial { n, p } => match (&**n, &**p) {
            (IntProvider::Constant(n), FloatProvider::Constant(p)) => {
                LootNumberProvider::Binomial {
                    n: *n,
                    probability: *p,
                }
            }
            _ => return None,
        },
        _ => return None,
    })
}

/// The plain [`LootNumberProvider`] form of a float provider, when it has one.
fn simple_float(provider: &FloatProvider) -> Option<LootNumberProvider> {
    Some(match provider {
        FloatProvider::Constant(value) => LootNumberProvider::Constant(f64::from(*value)),
        FloatProvider::Uniform { min, max } => match (&**min, &**max) {
            (FloatProvider::Constant(min), FloatProvider::Constant(max)) => {
                LootNumberProvider::Uniform {
                    min: f64::from(*min),
                    max: f64::from(*max),
                }
            }
            _ => return None,
        },
        _ => return None,
    })
}

impl<'a> Parser<'a> {
    fn new(resolver: &'a dyn LootResolver) -> Self {
        Self {
            resolver,
            depth: Cell::new(0),
        }
    }

    /// Parses the element a reference names, one level deeper.
    fn follow<T>(
        &self,
        value: Option<Value>,
        parse: impl FnOnce(&Value) -> Option<T>,
    ) -> Option<T> {
        let depth = self.depth.get();
        if depth >= MAX_PREDICATE_DEPTH {
            return None;
        }
        self.depth.set(depth + 1);
        let parsed = value.and_then(|value| parse(&value));
        self.depth.set(depth);
        parsed
    }

    /// A number provider read as an int (`ContextIntProviders.CODEC`).
    fn int_number(&self, value: &Value) -> Option<LootNumberProvider> {
        let provider = self.int(value)?;
        Some(simple_int(&provider).unwrap_or_else(|| LootNumberProvider::Int(provider.intern())))
    }

    /// A number provider read as a float (`ContextFloatProviders.CODEC`).
    fn float_number(&self, value: &Value) -> Option<LootNumberProvider> {
        let provider = self.float(value)?;
        Some(
            simple_float(&provider).unwrap_or_else(|| LootNumberProvider::Float(provider.intern())),
        )
    }

    fn ints(&self, value: &Value) -> Option<Vec<IntProvider>> {
        let inputs = many(value)
            .iter()
            .map(|input| self.int(input))
            .collect::<Option<Vec<_>>>()?;
        (!inputs.is_empty()).then_some(inputs)
    }

    fn floats(&self, value: &Value) -> Option<Vec<FloatProvider>> {
        let inputs = many(value)
            .iter()
            .map(|input| self.float(input))
            .collect::<Option<Vec<_>>>()?;
        (!inputs.is_empty()).then_some(inputs)
    }

    /// `WeightedList.nonEmptyCodec`: entries of `data` and `weight`.
    fn weighted<T>(value: &Value, parse: impl Fn(&Value) -> Option<T>) -> Option<Vec<(T, i32)>> {
        let entries = value
            .as_array()?
            .iter()
            .map(|entry| {
                let weight = entry
                    .get("weight")?
                    .as_i64()
                    .and_then(|w| i32::try_from(w).ok())?;
                (weight >= 0).then_some(())?;
                Some((parse(entry.get("data")?)?, weight))
            })
            .collect::<Option<Vec<_>>>()?;
        (!entries.is_empty()).then_some(entries)
    }

    fn dispatch_cases<T>(
        &self,
        value: &Value,
        parse: impl Fn(&Value) -> Option<T>,
    ) -> Option<Vec<(Condition, T)>> {
        value
            .get("cases")?
            .as_array()?
            .iter()
            .map(|case| {
                Some((
                    self.condition(case.get("condition")?, 0)?,
                    parse(case.get("value")?)?,
                ))
            })
            .collect()
    }

    /// `ContextIntProviders.CODEC`: an int, a typed provider or a `context_int_provider` id.
    #[allow(clippy::too_many_lines)]
    fn int(&self, value: &Value) -> Option<IntProvider> {
        if let Some(id) = value.as_str() {
            return self.follow(self.resolver.int_provider(&namespaced(id)), |value| {
                self.int(value)
            });
        }
        if let Some(number) = value.as_f64() {
            return Some(IntProvider::Constant(number as i32));
        }
        let field = |key: &str| self.int(value.get(key)?).map(Box::new);
        let float_field = |key: &str| self.float(value.get(key)?).map(Box::new);
        let fallback = || {
            value
                .get("fallback")
                .map_or(Some(IntProvider::Constant(0)), |fallback| {
                    self.int(fallback)
                })
                .map(Box::new)
        };
        Some(match kind(value) {
            "constant" => IntProvider::Constant(number(value.get("value")?)?),
            // Older packs leave the type out of a uniform provider.
            "uniform" | "" => IntProvider::Uniform {
                min: field("min")?,
                max: field("max")?,
            },
            "binomial" => IntProvider::Binomial {
                n: field("n")?,
                p: float_field("p")?,
            },
            "abs" => IntProvider::Abs(field("input")?),
            "negate" => IntProvider::Negate(field("input")?),
            "avg" => IntProvider::Average(self.ints(value.get("inputs")?)?),
            "max" => IntProvider::Max(self.ints(value.get("inputs")?)?),
            "min" => IntProvider::Min(self.ints(value.get("inputs")?)?),
            "mul" => IntProvider::Product(self.ints(value.get("inputs")?)?),
            "add" => IntProvider::Sum(self.ints(value.get("inputs")?)?),
            "sub" => IntProvider::Difference(field("left")?, field("right")?),
            "mod" => IntProvider::Modulus(field("left")?, field("right")?),
            "floor_mod" => IntProvider::FloorModulus(field("left")?, field("right")?),
            "div" => IntProvider::Quotient(field("left")?, field("right")?),
            "floor_div" => IntProvider::FloorQuotient(field("left")?, field("right")?),
            "pow" => IntProvider::Power {
                base: field("base")?,
                exponent: field("exponent")?,
            },
            "from_float" => IntProvider::FromFloat(float_field("input")?),
            "conditional" => IntProvider::Conditional {
                condition: self.condition(value.get("condition")?, 0)?,
                on_true: field("on_true")?,
                on_false: value
                    .get("on_false")
                    .map_or(Some(IntProvider::Constant(0)), |on_false| {
                        self.int(on_false)
                    })
                    .map(Box::new)?,
            },
            "number_dispatcher" => IntProvider::Dispatcher {
                cases: self.dispatch_cases(value, |value| self.int(value))?,
                default: value
                    .get("default")
                    .map_or(Some(IntProvider::Constant(0)), |default| self.int(default))
                    .map(Box::new)?,
            },
            "weighted_list" => {
                IntProvider::Weighted(Self::weighted(value.get("distribution")?, |value| {
                    self.int(value)
                })?)
            }
            "score" => IntProvider::Score {
                target: score_target(value.get("target")?)?,
                score: value.get("score")?.as_str()?.to_owned(),
                fallback: fallback()?,
            },
            "storage" => IntProvider::Storage {
                access: stored_number(value)?,
                fallback: fallback()?,
            },
            "environment_attribute" => {
                IntProvider::EnvironmentAttribute(environment_attribute(value)?)
            }
            _ => return None,
        })
    }

    /// `ContextFloatProviders.CODEC`: a float, a typed provider or a `context_float_provider` id.
    fn float(&self, value: &Value) -> Option<FloatProvider> {
        if let Some(id) = value.as_str() {
            return self.follow(self.resolver.float_provider(&namespaced(id)), |value| {
                self.float(value)
            });
        }
        if let Some(number) = value.as_f64() {
            return Some(FloatProvider::Constant(number as f32));
        }
        let field = |key: &str| self.float(value.get(key)?).map(Box::new);
        let or_zero = |key: &str| {
            value
                .get(key)
                .map_or(Some(FloatProvider::Constant(0.0)), |value| {
                    self.float(value)
                })
                .map(Box::new)
        };
        Some(match kind(value) {
            "constant" => FloatProvider::Constant(value.get("value")?.as_f64()? as f32),
            "uniform" | "" => FloatProvider::Uniform {
                min: field("min")?,
                max: field("max")?,
            },
            "abs" => FloatProvider::Abs(field("input")?),
            "negate" => FloatProvider::Negate(field("input")?),
            "ceil" => FloatProvider::Ceiling(field("input")?),
            "floor" => FloatProvider::Floor(field("input")?),
            "round" => FloatProvider::Round(field("input")?),
            "truncate" => FloatProvider::Truncate(field("input")?),
            "sin" => FloatProvider::Sine(field("input")?),
            "cos" => FloatProvider::Cosine(field("input")?),
            "sqrt" => FloatProvider::SquareRoot(field("input")?),
            "avg" => FloatProvider::Average(self.floats(value.get("inputs")?)?),
            "length" => FloatProvider::Length(self.floats(value.get("inputs")?)?),
            "max" => FloatProvider::Max(self.floats(value.get("inputs")?)?),
            "min" => FloatProvider::Min(self.floats(value.get("inputs")?)?),
            "mul" => FloatProvider::Product(self.floats(value.get("inputs")?)?),
            "add" => FloatProvider::Sum(self.floats(value.get("inputs")?)?),
            "sub" => FloatProvider::Difference(field("left")?, field("right")?),
            "mod" => FloatProvider::Modulus(field("left")?, field("right")?),
            "div" => FloatProvider::Quotient(field("left")?, field("right")?),
            "pow" => FloatProvider::Power {
                base: field("base")?,
                exponent: field("exponent")?,
            },
            "from_int" => FloatProvider::FromInt(Box::new(self.int(value.get("input")?)?)),
            "enchantment_level" => {
                FloatProvider::EnchantmentLevel(level_based(value.get("amount")?)?)
            }
            "conditional" => FloatProvider::Conditional {
                condition: self.condition(value.get("condition")?, 0)?,
                on_true: field("on_true")?,
                on_false: or_zero("on_false")?,
            },
            "number_dispatcher" => FloatProvider::Dispatcher {
                cases: self.dispatch_cases(value, |value| self.float(value))?,
                default: or_zero("default")?,
            },
            "weighted_list" => {
                FloatProvider::Weighted(Self::weighted(value.get("distribution")?, |value| {
                    self.float(value)
                })?)
            }
            "storage" => FloatProvider::Storage {
                access: stored_number(value)?,
                fallback: or_zero("fallback")?,
            },
            "environment_attribute" => {
                FloatProvider::EnvironmentAttribute(environment_attribute(value)?)
            }
            _ => return None,
        })
    }

    /// A `random_chance` condition (`LootItemRandomChanceCondition`). A constant chance and an
    /// `enchantment_level` chance with a constant or linear amount keep their own forms.
    fn random_chance(&self, chance: Option<&Value>) -> Condition {
        let Some(chance) = chance else {
            return Condition::RandomChance { chance: 0.0 };
        };
        if let Some(chance) = chance.as_f64() {
            return Condition::RandomChance {
                chance: chance as f32,
            };
        }
        let amount = (kind(chance) == "enchantment_level")
            .then(|| chance.get("amount"))
            .flatten();
        let level_based = amount.and_then(|amount| {
            if let Some(constant) = amount.as_f64() {
                return Some((constant, 0.0));
            }
            (kind(amount) == "linear").then_some((
                amount.get("base")?.as_f64()?,
                amount.get("per_level_above_first")?.as_f64()?,
            ))
        });
        if let Some((base, per_level_above_first)) = level_based {
            return Condition::RandomChanceWithEnchantmentLevel {
                base: base as f32,
                per_level_above_first: per_level_above_first as f32,
            };
        }
        self.float_number(chance).map_or(
            Condition::RandomChance { chance: 0.0 },
            Condition::RandomChanceNumber,
        )
    }

    fn table(&self, value: &Value) -> Option<Table> {
        value.as_object()?;
        let pools = match value.get("pools") {
            Some(pools) => pools
                .as_array()?
                .iter()
                .map(|pool| self.pool(pool))
                .collect::<Option<_>>()?,
            None => Vec::new(),
        };
        Some(Table {
            pools,
            functions: self.functions(value),
        })
    }

    fn pool(&self, value: &Value) -> Option<Pool> {
        let entries = value
            .get("entries")
            .and_then(Value::as_array)?
            .iter()
            .map(|entry| self.entry(entry))
            .collect::<Option<_>>()?;
        let rolls = value
            .get("rolls")
            .map_or(Some(LootNumberProvider::Constant(1.0)), |rolls| {
                self.int_number(rolls)
            })?;
        let bonus_rolls = value
            .get("bonus_rolls")
            .map_or(Some(LootNumberProvider::Constant(0.0)), |bonus_rolls| {
                self.float_number(bonus_rolls)
            })?;
        Some(Pool {
            entries,
            rolls,
            bonus_rolls,
            condition: self.conditions(value),
            functions: self.functions(value),
        })
    }

    fn entry(&self, value: &Value) -> Option<Entry> {
        let children = || -> Option<Vec<Entry>> {
            match value.get("children") {
                Some(children) => children
                    .as_array()?
                    .iter()
                    .map(|child| self.entry(child))
                    .collect(),
                None => Some(Vec::new()),
            }
        };
        let kind = match kind(value) {
            "item" => EntryKind::Item(value.get("name")?.as_str()?.to_owned()),
            "dynamic" => EntryKind::Dynamic(value.get("name")?.as_str()?.to_owned()),
            "empty" => EntryKind::Empty,
            "alternatives" => EntryKind::Alternatives(children()?),
            "group" => EntryKind::Group(children()?),
            "sequence" => EntryKind::Sequence(children()?),
            "tag" => EntryKind::Tag {
                name: value
                    .get("items")
                    .or_else(|| value.get("name"))?
                    .as_str()?
                    .to_owned(),
                expand: value
                    .get("expand")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            },
            "slots" => EntryKind::Slots(self.slot_source(value.get("slot_source")?)?),
            "loot_table" => {
                let table = value.get("value").or_else(|| value.get("name"))?;
                if let Some(name) = table.as_str() {
                    EntryKind::Table(name.to_owned())
                } else {
                    EntryKind::InlineTable(Box::new(self.table(table)?))
                }
            }
            _ => return None,
        };
        Some(Entry {
            kind,
            weight: value
                .get("weight")
                .and_then(Value::as_i64)
                .and_then(|weight| i32::try_from(weight).ok())
                .unwrap_or(1),
            quality: value
                .get("quality")
                .and_then(Value::as_i64)
                .and_then(|value| i32::try_from(value).ok())
                .unwrap_or(0),
            condition: self.conditions(value),
            functions: self.functions(value),
        })
    }

    fn conditions(&self, value: &Value) -> Condition {
        value
            .get("condition")
            .or_else(|| value.get("conditions"))
            .map_or(Condition::None, |value| {
                self.condition(value, 0).unwrap_or(Condition::Never)
            })
    }

    #[allow(clippy::too_many_lines)]
    fn condition(&self, value: &Value, depth: usize) -> Option<Condition> {
        if depth >= MAX_PREDICATE_DEPTH {
            return None;
        }
        if let Some(key) = value.as_str() {
            return self.follow(self.resolver.predicate(&namespaced(key)), |value| {
                self.condition(value, depth + 1)
            });
        }
        if let Some(conditions) = value.as_array() {
            return Some(all(conditions
                .iter()
                .map(|condition| self.condition(condition, depth + 1))
                .collect::<Option<_>>()?));
        }
        Some(match kind(value) {
            "survives_explosion" => Condition::SurvivesExplosion,
            "killed_by_player" => Condition::KilledByPlayer,
            "random_chance" => self.random_chance(value.get("chance")),
            "random_chance_with_enchanted_bonus" => {
                let unenchanted_chance = value
                    .get("unenchanted_chance")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0) as f32;
                let chance = value.get("enchanted_chance");
                let enchanted_chance_base = chance
                    .and_then(|chance| {
                        chance
                            .as_f64()
                            .or_else(|| chance.get("base").and_then(Value::as_f64))
                    })
                    .map_or(unenchanted_chance, |number| number as f32);
                let enchanted_chance_per_level_above_first = chance
                    .and_then(|chance| chance.get("per_level_above_first"))
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0)
                    as f32;
                Condition::RandomChanceWithEnchantedBonus {
                    enchantment: value
                        .get("enchantment")
                        .and_then(Value::as_str)
                        .unwrap_or("minecraft:looting")
                        .to_owned(),
                    unenchanted_chance,
                    enchanted_chance_base,
                    enchanted_chance_per_level_above_first,
                }
            }
            "random_chance_with_looting" => {
                let chance = value.get("chance").and_then(Value::as_f64).unwrap_or(0.0) as f32;
                let looting_multiplier = value
                    .get("looting_multiplier")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0) as f32;
                Condition::RandomChanceWithLooting {
                    chance,
                    looting_multiplier,
                }
            }
            "table_bonus" => Condition::TableBonus {
                enchantment: value
                    .get("enchantment")
                    .and_then(Value::as_str)
                    .unwrap_or("minecraft:fortune")
                    .to_owned(),
                chances: value
                    .get("chances")
                    .and_then(Value::as_array)
                    .map(|values| {
                        values
                            .iter()
                            .filter_map(Value::as_f64)
                            .map(|value| value as f32)
                            .collect()
                    })
                    .unwrap_or_default(),
            },
            "all_of" => all(value
                .get("terms")?
                .as_array()?
                .iter()
                .map(|term| self.condition(term, depth + 1))
                .collect::<Option<_>>()?),
            "any_of" => Condition::AnyOf(
                value
                    .get("terms")?
                    .as_array()?
                    .iter()
                    .map(|term| self.condition(term, depth + 1))
                    .collect::<Option<_>>()?,
            ),
            "inverted" => {
                Condition::Inverted(Box::new(self.condition(value.get("term")?, depth + 1)?))
            }
            "entity_properties" => Self::entity_properties(value)?,
            "damage_source_properties" => Self::damage_source_properties(value),
            "location_check" => Self::location_check(value)?,
            "match_block" | "block_state_property" => {
                Condition::MatchBlock(value.get("predicate").unwrap_or(value).clone())
            }
            "match_tool" => Condition::MatchTool(
                value
                    .get("predicate")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({})),
            ),
            "weather_check" => Condition::WeatherCheck {
                raining: value.get("raining").and_then(Value::as_bool),
                thundering: value.get("thundering").and_then(Value::as_bool),
            },
            "time_check" => {
                let period = value.get("period").and_then(Value::as_u64);
                let value = value.get("value").cloned().unwrap_or(Value::Null);
                Condition::TimeCheck { value, period }
            }
            "value_check" => {
                let val = self.float_number(value.get("value")?)?;
                let range = value.get("range").cloned().unwrap_or(Value::Null);
                Condition::ValueCheck { value: val, range }
            }
            "reference" => {
                let name = value.get("name").and_then(Value::as_str)?;
                self.follow(self.resolver.predicate(&namespaced(name)), |value| {
                    self.condition(value, depth + 1)
                })
                .unwrap_or_else(|| Condition::Reference(name.to_owned()))
            }
            _ => return None,
        })
    }

    fn location_check(value: &Value) -> Option<Condition> {
        let offset = |key| {
            value.get(key).map_or(Some(0), |value| {
                value.as_i64().and_then(|value| i32::try_from(value).ok())
            })
        };
        Some(Condition::LocationCheck {
            offset: [offset("offsetX")?, offset("offsetY")?, offset("offsetZ")?],
            predicate: value
                .get("predicate")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({})),
        })
    }

    fn entity_properties(value: &Value) -> Option<Condition> {
        let target = match value.get("entity").and_then(Value::as_str) {
            Some("this") => LootEntityTarget::This,
            Some("attacker" | "killer") => LootEntityTarget::Attacker,
            Some("direct_attacker" | "direct_killer") => LootEntityTarget::DirectAttacker,
            _ => return None,
        };
        let mut predicate = value.get("predicate")?.as_object()?.clone();
        let mut conditions = Vec::new();
        if target == LootEntityTarget::This {
            for key in ["minecraft:flags", "flags"] {
                if let Some(flags) = predicate.get_mut(key).and_then(Value::as_object_mut)
                    && flags.get("is_on_fire").and_then(Value::as_bool) == Some(true)
                {
                    conditions.push(Condition::EntityOnFire);
                    flags.remove("is_on_fire");
                    if flags.is_empty() {
                        predicate.remove(key);
                    }
                }
            }
        }
        if target == LootEntityTarget::DirectAttacker {
            for key in ["minecraft:equipment", "equipment"] {
                if let Some(tag) = predicate
                    .get(key)
                    .and_then(|equipment| {
                        equipment
                            .pointer("/mainhand/predicates/minecraft:enchantments/0/enchantments")
                    })
                    .and_then(Value::as_str)
                    .and_then(|tag| tag.strip_prefix('#'))
                {
                    conditions.push(Condition::DirectAttackerWeaponEnchantment(tag.to_owned()));
                    predicate.remove(key);
                }
            }
        }
        if !predicate.is_empty() || conditions.is_empty() {
            conditions.push(Condition::EntityProperties {
                target,
                predicate: Value::Object(predicate),
            });
        }
        Some(all(conditions))
    }

    fn damage_source_properties(value: &Value) -> Condition {
        let Some(predicate) = value.get("predicate") else {
            return Condition::Never;
        };
        let mut conditions = Vec::new();
        for (field, target) in [
            ("source_entity", LootEntityTarget::Attacker),
            ("direct_entity", LootEntityTarget::DirectAttacker),
        ] {
            if let Some(predicate) = predicate.get(field) {
                conditions.push(Condition::EntityProperties {
                    target,
                    predicate: predicate.clone(),
                });
            }
        }
        if let Some(tags) = predicate.get("tags").and_then(Value::as_array) {
            for tag in tags {
                if let Some(name) = tag.get("id").and_then(Value::as_str) {
                    conditions.push(Condition::DamageSourceTag {
                        tag: name.trim_start_matches('#').to_owned(),
                        expected: tag.get("expected").and_then(Value::as_bool).unwrap_or(true),
                    });
                }
            }
        }
        for (flag, tag_name) in [
            ("is_projectile", "minecraft:is_projectile"),
            ("is_explosion", "minecraft:is_explosion"),
            ("is_fire", "minecraft:is_fire"),
            ("is_lightning", "minecraft:is_lightning"),
            ("is_fall", "minecraft:is_fall"),
        ] {
            if let Some(expected) = predicate.get(flag).and_then(Value::as_bool) {
                conditions.push(Condition::DamageSourceTag {
                    tag: tag_name.to_owned(),
                    expected,
                });
            }
        }
        all(conditions)
    }

    fn functions(&self, value: &Value) -> Vec<Function> {
        value
            .get("modifier")
            .or_else(|| value.get("functions"))
            .map_or_else(Vec::new, |functions| self.function_list(functions))
    }

    /// `LootItemFunctions.CODEC`: a function, an inline list of them or an `item_modifier` id.
    fn function_list(&self, functions: &Value) -> Vec<Function> {
        let mut parsed = Vec::new();
        for function in many(functions) {
            if let Some(id) = function.as_str() {
                let modifier = self.follow(self.resolver.item_modifier(&namespaced(id)), |value| {
                    Some(self.function_list(value))
                });
                parsed.extend(modifier.unwrap_or_default());
            } else if let Some(function) = self.function(function) {
                parsed.push(function);
            }
        }
        parsed
    }

    fn function(&self, function: &Value) -> Option<Function> {
        let kind = match kind(function) {
            "set_count" => {
                let count = self.int_number(function.get("count")?)?;
                LootFunctionKind::SetCount {
                    count,
                    add: function
                        .get("add")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                }
            }
            "furnace_smelt" => LootFunctionKind::FurnaceSmelt,
            "apply_bonus" => {
                let formula = function.get("formula").and_then(Value::as_str)?;
                let parameters = function.get("parameters");
                let formula = match formula.trim_start_matches("minecraft:") {
                    "ore_drops" => LootBonusFormula::OreDrops,
                    "uniform_bonus_count" => LootBonusFormula::UniformBonusCount(
                        parameters
                            .and_then(|parameters| parameters.get("bonusMultiplier"))
                            .and_then(number)
                            .unwrap_or(1),
                    ),
                    "binomial_with_bonus_count" => LootBonusFormula::BinomialWithBonusCount {
                        extra: parameters
                            .and_then(|parameters| parameters.get("extra"))
                            .and_then(number)
                            .unwrap_or(0),
                        probability: parameters
                            .and_then(|parameters| parameters.get("probability"))
                            .and_then(Value::as_f64)
                            .unwrap_or(0.0) as f32,
                    },
                    _ => return None,
                };
                LootFunctionKind::ApplyBonus(formula)
            }
            "enchanted_count_increase" | "looting_enchant" => {
                LootFunctionKind::EnchantedCountIncrease {
                    count: self.float_number(function.get("count")?)?,
                    limit: function.get("limit").and_then(number).unwrap_or(0),
                }
            }
            "limit_count" => {
                let limit = function.get("limit")?;
                let (min, max) = if let Some(value) = scalar(limit) {
                    (
                        Some(LootNumberProvider::Constant(value)),
                        Some(LootNumberProvider::Constant(value)),
                    )
                } else {
                    (
                        match limit.get("min") {
                            Some(value) => Some(self.int_number(value)?),
                            None => None,
                        },
                        match limit.get("max") {
                            Some(value) => Some(self.int_number(value)?),
                            None => None,
                        },
                    )
                };
                LootFunctionKind::LimitCount { min, max }
            }
            "explosion_decay" => LootFunctionKind::ExplosionDecay,
            name if JSON_LOOT_FUNCTIONS.contains(&name) => LootFunctionKind::Json,
            _ => return None,
        };
        let enchantment = function
            .get("enchantment")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| match kind {
                LootFunctionKind::ApplyBonus(_) => Some("minecraft:fortune".to_owned()),
                LootFunctionKind::EnchantedCountIncrease { .. } => {
                    Some("minecraft:looting".to_owned())
                }
                _ => None,
            });
        Some(Function {
            kind,
            enchantment,
            condition: self.conditions(function),
            json: (kind == LootFunctionKind::Json).then(|| function.clone()),
        })
    }

    /// `SlotSources.CODEC`: a typed slot source, an inline group or a `slot_source` id.
    fn slot_source(&self, value: &Value) -> Option<SlotSource> {
        if let Some(id) = value.as_str() {
            return self.follow(self.resolver.slot_source(&namespaced(id)), |value| {
                self.slot_source(value)
            });
        }
        if let Some(terms) = value.as_array() {
            return self.slot_source_terms(terms).map(SlotSource::Group);
        }
        let inner = || self.slot_source(value.get("slot_source")?).map(Box::new);
        Some(match kind(value) {
            "empty" => SlotSource::Empty,
            "group" => SlotSource::Group(self.slot_source_terms(value.get("terms")?.as_array()?)?),
            "slot_range" => SlotSource::Range {
                source: match value.get("source") {
                    None => SlotSourceTarget::Container,
                    Some(source) => match source.as_str()? {
                        "container" => SlotSourceTarget::Container,
                        "block_entity" => SlotSourceTarget::BlockEntity,
                        name => SlotSourceTarget::Entity(score_entity(name)?),
                    },
                },
                slots: value.get("slots")?.as_str()?.to_owned(),
            },
            "filtered" => SlotSource::Filtered {
                source: inner()?,
                item_filter: value.get("item_filter")?.clone(),
            },
            "limit_slots" => {
                let limit = value.get("limit")?.as_i64()?;
                SlotSource::Limit {
                    source: inner()?,
                    limit: usize::try_from(limit).ok().filter(|limit| *limit > 0)?,
                }
            }
            "contents" => {
                let component = value.get("component")?.as_str()?;
                let component = component.strip_prefix("minecraft:").unwrap_or(component);
                SlotSource::Contents {
                    source: inner()?,
                    component: match component {
                        "container" | "bundle_contents" | "charged_projectiles" => {
                            component.to_owned()
                        }
                        _ => return None,
                    },
                }
            }
            _ => return None,
        })
    }

    /// `SlotSources.LIST_CODEC`: typed slot sources or `slot_source` ids.
    fn slot_source_terms(&self, terms: &[Value]) -> Option<Vec<SlotSource>> {
        terms.iter().map(|term| self.slot_source(term)).collect()
    }
}
