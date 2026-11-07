//! The `ContextIntProvider` and `ContextFloatProvider` types of 26.3 that a
//! [`LootNumberProvider`](super::LootNumberProvider) can't express with its plain variants.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use super::{DynamicLootCondition, LootEntityTarget};

/// Who a `score` provider reads the score of (`ScoreboardNameProvider`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScoreTarget {
    /// `ContextScoreboardNameProvider`: an entity of the loot context.
    Entity(ScoreEntity),
    /// `FixedScoreboardNameProvider`: a score holder name.
    Fixed(String),
}

/// `LootContext.EntityTarget`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScoreEntity {
    Target(LootEntityTarget),
    AttackingPlayer,
    TargetEntity,
    InteractingEntity,
}

/// `StoredNumberAccess`: a numeric tag in command storage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredNumber {
    pub storage: String,
    pub path: String,
}

/// `LevelBasedValue`.
#[derive(Clone, Debug, PartialEq)]
pub enum LevelBased {
    Constant(f32),
    Linear {
        base: f32,
        per_level_above_first: f32,
    },
    Clamped {
        value: Box<Self>,
        min: f32,
        max: f32,
    },
    Fraction {
        numerator: Box<Self>,
        denominator: Box<Self>,
    },
    LevelsSquared {
        added: f32,
    },
    Exponent {
        base: Box<Self>,
        power: Box<Self>,
    },
    Lookup {
        values: Vec<f32>,
        fallback: Box<Self>,
    },
}

impl LevelBased {
    /// `LevelBasedValue.calculate`.
    #[must_use]
    pub fn calculate(&self, level: i32) -> f32 {
        match self {
            Self::Constant(value) => *value,
            Self::Linear {
                base,
                per_level_above_first,
            } => base + per_level_above_first * (level - 1) as f32,
            Self::Clamped { value, min, max } => value.calculate(level).clamp(*min, *max),
            Self::Fraction {
                numerator,
                denominator,
            } => {
                let denominator = denominator.calculate(level);
                if denominator == 0.0 {
                    0.0
                } else {
                    numerator.calculate(level) / denominator
                }
            }
            Self::LevelsSquared { added } => (level * level) as f32 + added,
            Self::Exponent { base, power } => {
                f64::from(base.calculate(level)).powf(f64::from(power.calculate(level))) as f32
            }
            Self::Lookup { values, fallback } => usize::try_from(level - 1)
                .ok()
                .and_then(|index| values.get(index))
                .map_or_else(|| fallback.calculate(level), |value| *value),
        }
    }
}

/// A `ContextIntProvider`, with references to the `context_int_provider` registry resolved.
#[derive(Clone, Debug, PartialEq)]
pub enum IntProvider {
    Constant(i32),
    Uniform {
        min: Box<Self>,
        max: Box<Self>,
    },
    Binomial {
        n: Box<Self>,
        p: Box<FloatProvider>,
    },
    Abs(Box<Self>),
    Negate(Box<Self>),
    Average(Vec<Self>),
    Max(Vec<Self>),
    Min(Vec<Self>),
    Product(Vec<Self>),
    Sum(Vec<Self>),
    Difference(Box<Self>, Box<Self>),
    Modulus(Box<Self>, Box<Self>),
    FloorModulus(Box<Self>, Box<Self>),
    Quotient(Box<Self>, Box<Self>),
    FloorQuotient(Box<Self>, Box<Self>),
    Power {
        base: Box<Self>,
        exponent: Box<Self>,
    },
    FromFloat(Box<FloatProvider>),
    Conditional {
        condition: DynamicLootCondition,
        on_true: Box<Self>,
        on_false: Box<Self>,
    },
    Dispatcher {
        cases: Vec<(DynamicLootCondition, Self)>,
        default: Box<Self>,
    },
    Weighted(Vec<(Self, i32)>),
    Score {
        target: ScoreTarget,
        score: String,
        fallback: Box<Self>,
    },
    Storage {
        access: StoredNumber,
        fallback: Box<Self>,
    },
    EnvironmentAttribute(String),
}

/// A `ContextFloatProvider`, with references to the `context_float_provider` registry resolved.
#[derive(Clone, Debug, PartialEq)]
pub enum FloatProvider {
    Constant(f32),
    Uniform {
        min: Box<Self>,
        max: Box<Self>,
    },
    Abs(Box<Self>),
    Negate(Box<Self>),
    Ceiling(Box<Self>),
    Floor(Box<Self>),
    Round(Box<Self>),
    Truncate(Box<Self>),
    Sine(Box<Self>),
    Cosine(Box<Self>),
    SquareRoot(Box<Self>),
    Average(Vec<Self>),
    Length(Vec<Self>),
    Max(Vec<Self>),
    Min(Vec<Self>),
    Product(Vec<Self>),
    Sum(Vec<Self>),
    Difference(Box<Self>, Box<Self>),
    Modulus(Box<Self>, Box<Self>),
    Quotient(Box<Self>, Box<Self>),
    Power {
        base: Box<Self>,
        exponent: Box<Self>,
    },
    FromInt(Box<IntProvider>),
    EnchantmentLevel(LevelBased),
    Conditional {
        condition: DynamicLootCondition,
        on_true: Box<Self>,
        on_false: Box<Self>,
    },
    Dispatcher {
        cases: Vec<(DynamicLootCondition, Self)>,
        default: Box<Self>,
    },
    Weighted(Vec<(Self, i32)>),
    Storage {
        access: StoredNumber,
        fallback: Box<Self>,
    },
    EnvironmentAttribute(String),
}

/// One copy of each distinct provider, so that loot numbers stay `Copy`. A provider is only
/// added when a loot table or predicate with it loads, and the same definition is reused on
/// every reload.
fn intern<T: std::fmt::Debug + Send + Sync>(
    cache: &Mutex<HashMap<String, &'static T>>,
    provider: T,
) -> &'static T {
    let key = format!("{provider:?}");
    let mut cache = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    cache
        .entry(key)
        .or_insert_with(|| Box::leak(Box::new(provider)))
}

static INT_PROVIDERS: LazyLock<Mutex<HashMap<String, &'static IntProvider>>> =
    LazyLock::new(Mutex::default);
static FLOAT_PROVIDERS: LazyLock<Mutex<HashMap<String, &'static FloatProvider>>> =
    LazyLock::new(Mutex::default);

impl IntProvider {
    #[must_use]
    pub fn intern(self) -> &'static Self {
        intern(&INT_PROVIDERS, self)
    }
}

impl FloatProvider {
    #[must_use]
    pub fn intern(self) -> &'static Self {
        intern(&FLOAT_PROVIDERS, self)
    }
}
