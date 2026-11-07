//! `ContextIntProvider.getInt` and `ContextFloatProvider.getFloat` for loot number providers.

use pumpkin_data::environment_attribute::EnvironmentAttribute;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_util::loot_table::number::{
    FloatProvider, IntProvider, ScoreEntity, ScoreTarget, StoredNumber,
};
use pumpkin_util::loot_table::{DynamicLootCondition, LootEntityTarget, LootNumberProvider};
use pumpkin_util::random::{RandomGenerator, RandomImpl};

use super::{LootContextParameters, check_dynamic_condition};
use crate::command::CommandSource;
use crate::command::argument_types::argument_type::ArgumentType;
use crate::command::argument_types::nbt_path::NbtPathArgumentType;
use crate::command::string_reader::StringReader;
use crate::entity::EntityBase;
use crate::world::environment::EnvironmentAttributes;

/// An `ArithmeticException` of `getIntUnsafe` or `getFloatUnsafe`: the outermost provider
/// gives 0 instead.
struct ArithmeticError;

type Number<T> = Result<T, ArithmeticError>;

/// What a number provider reads: the loot context and its random.
pub(super) struct NumberContext<'a, 'b> {
    pub params: &'a LootContextParameters<'b>,
    pub rng: &'a mut RandomGenerator,
    pub has_silk_touch: bool,
    pub has_shears: bool,
}

/// `ContextIntProvider.longToIntSafe`.
fn long_to_int(value: i64) -> Number<i32> {
    i32::try_from(value).map_err(|_| ArithmeticError)
}

/// `ContextIntProvider.floatToIntSafe`.
fn float_to_int(value: f32) -> Number<i32> {
    if value.is_finite() {
        long_to_int(value as i64)
    } else {
        Err(ArithmeticError)
    }
}

/// `ContextFloatProvider.getFloatOrThrow`.
const fn finite(value: f32) -> Number<f32> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(ArithmeticError)
    }
}

/// The entity of a `LootContext.EntityTarget`.
pub(super) fn context_entity<'a>(
    params: &LootContextParameters<'a>,
    target: ScoreEntity,
) -> Option<&'a dyn EntityBase> {
    match target {
        ScoreEntity::Target(LootEntityTarget::This) => params.this_entity,
        ScoreEntity::Target(LootEntityTarget::Attacker) => params.killer_entity,
        ScoreEntity::Target(LootEntityTarget::DirectAttacker) => params.direct_killer_entity,
        // `LAST_DAMAGE_PLAYER`: Pumpkin only knows the killer.
        ScoreEntity::AttackingPlayer => params
            .killer_entity
            .filter(|entity| entity.get_player().is_some()),
        ScoreEntity::TargetEntity | ScoreEntity::InteractingEntity => None,
    }
}

impl NumberContext<'_, '_> {
    /// `ContextIntProvider.getInt` for an int number.
    pub fn int(&mut self, provider: LootNumberProvider) -> i32 {
        match provider {
            LootNumberProvider::Constant(value) => value as i32,
            LootNumberProvider::Uniform { min, max } => self.between(min as i32, max as i32),
            LootNumberProvider::Binomial { n, probability } => self.binomial(n, probability),
            LootNumberProvider::Int(provider) => self.int_unsafe(provider).unwrap_or(0),
            LootNumberProvider::Float(provider) => self
                .float_unsafe(provider)
                .and_then(float_to_int)
                .unwrap_or(0),
        }
    }

    /// `ContextFloatProvider.getFloat` for a float number.
    pub fn float(&mut self, provider: LootNumberProvider) -> f32 {
        match provider {
            LootNumberProvider::Constant(value) => value as f32,
            LootNumberProvider::Uniform { min, max } => self.between_float(min as f32, max as f32),
            LootNumberProvider::Binomial { n, probability } => self.binomial(n, probability) as f32,
            LootNumberProvider::Int(provider) => {
                self.int_unsafe(provider).map_or(0.0, |v| v as f32)
            }
            LootNumberProvider::Float(provider) => self
                .float_unsafe(provider)
                .ok()
                .filter(|value| value.is_finite())
                .unwrap_or(0.0),
        }
    }

    /// `Mth.nextInt`.
    fn between(&mut self, min: i32, max: i32) -> i32 {
        if min >= max {
            return min;
        }
        min + self.rng.next_bounded_i32(max - min + 1)
    }

    /// `Mth.nextFloat`.
    fn between_float(&mut self, min: f32, max: f32) -> f32 {
        if min >= max {
            min
        } else {
            self.rng.next_f32() * (max - min) + min
        }
    }

    fn binomial(&mut self, n: i32, probability: f32) -> i32 {
        (0..n).filter(|_| self.rng.next_f32() < probability).count() as i32
    }

    fn test(&mut self, condition: &DynamicLootCondition) -> bool {
        check_dynamic_condition(
            condition,
            self.has_silk_touch,
            self.has_shears,
            self.params,
            self.rng,
        )
    }

    /// `WeightedList.getRandomOrThrow`.
    fn weighted<'p, T>(&mut self, entries: &'p [(T, i32)]) -> Number<&'p T> {
        let total = entries
            .iter()
            .try_fold(0i32, |total, (_, weight)| total.checked_add(*weight))
            .ok_or(ArithmeticError)?;
        if total <= 0 {
            return Err(ArithmeticError);
        }
        let mut pick = self.rng.next_bounded_i32(total);
        for (value, weight) in entries {
            pick -= weight;
            if pick < 0 {
                return Ok(value);
            }
        }
        Err(ArithmeticError)
    }

    /// `StoredNumberAccess.getNumericTag`: the single numeric tag at the path.
    fn stored(&self, access: &StoredNumber) -> Option<NbtTag> {
        let server = self.params.world?.server.upgrade()?;
        let storage = server
            .command_storage
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&access.storage)
            .cloned()
            .unwrap_or_default();
        let mut reader = StringReader::new(&access.path);
        let path = ArgumentType::<CommandSource>::parse(&NbtPathArgumentType, &mut reader).ok()?;
        let mut tags = path.get(&NbtTag::Compound(storage)).ok()?;
        if tags.len() != 1 {
            return None;
        }
        let tag = tags.pop()?;
        matches!(
            tag,
            NbtTag::Byte(_)
                | NbtTag::Short(_)
                | NbtTag::Int(_)
                | NbtTag::Long(_)
                | NbtTag::Float(_)
                | NbtTag::Double(_)
        )
        .then_some(tag)
    }

    /// `ScoreboardValue.getScoreInfo`: the score, if the holder has one.
    fn score(&self, target: &ScoreTarget, objective: &str) -> Option<i32> {
        let holder = match target {
            ScoreTarget::Fixed(name) => name.clone(),
            ScoreTarget::Entity(target) => {
                context_entity(self.params, *target)?.get_scoreboard_name()
            }
        };
        let world = self.params.world?;
        let scoreboard = world
            .scoreboard
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        scoreboard.get_objective(objective)?;
        scoreboard.get_score_value(&holder, objective)
    }

    /// `EnvironmentAttributeValue`: the attributes Pumpkin evaluates for the whole dimension.
    fn environment_attribute(&self, name: &str) -> f32 {
        let (Some(world), Some(attribute)) =
            (self.params.world, EnvironmentAttribute::from_name(name))
        else {
            return 0.0;
        };
        EnvironmentAttributes::new(world).get_dimension_value_f32(attribute)
    }

    #[allow(clippy::too_many_lines)]
    fn int_unsafe(&mut self, provider: &IntProvider) -> Number<i32> {
        Ok(match provider {
            IntProvider::Constant(value) => *value,
            IntProvider::Uniform { min, max } => {
                let (min, max) = (self.int_unsafe(min)?, self.int_unsafe(max)?);
                self.between(min, max)
            }
            IntProvider::Binomial { n, p } => {
                let n = self.int_unsafe(n)?;
                let p = finite(self.float_unsafe(p)?)?;
                self.binomial(n, p)
            }
            IntProvider::Abs(input) => self
                .int_unsafe(input)?
                .checked_abs()
                .ok_or(ArithmeticError)?,
            IntProvider::Negate(input) => self
                .int_unsafe(input)?
                .checked_neg()
                .ok_or(ArithmeticError)?,
            IntProvider::Average(inputs) => {
                let mut sum = 0i64;
                for input in inputs {
                    sum += i64::from(self.int_unsafe(input)?);
                }
                long_to_int(sum / inputs.len() as i64)?
            }
            IntProvider::Max(inputs) => {
                let mut value = i32::MIN;
                for input in inputs {
                    value = value.max(self.int_unsafe(input)?);
                }
                value
            }
            IntProvider::Min(inputs) => {
                let mut value = i32::MAX;
                for input in inputs {
                    value = value.min(self.int_unsafe(input)?);
                }
                value
            }
            IntProvider::Product(inputs) => {
                let mut value = 1i64;
                for input in inputs {
                    value = value.wrapping_mul(i64::from(self.int_unsafe(input)?));
                }
                long_to_int(value)?
            }
            IntProvider::Sum(inputs) => {
                let mut value = 0i64;
                for input in inputs {
                    value += i64::from(self.int_unsafe(input)?);
                }
                long_to_int(value)?
            }
            IntProvider::Difference(left, right) => self
                .int_unsafe(left)?
                .checked_sub(self.int_unsafe(right)?)
                .ok_or(ArithmeticError)?,
            // Java's `%` and `/` only throw for a zero divisor; `MIN_VALUE / -1` wraps.
            IntProvider::Modulus(left, right) => {
                let (left, right) = (self.int_unsafe(left)?, self.int_unsafe(right)?);
                if right == 0 {
                    return Err(ArithmeticError);
                }
                left.wrapping_rem(right)
            }
            IntProvider::Quotient(left, right) => {
                let (left, right) = (self.int_unsafe(left)?, self.int_unsafe(right)?);
                if right == 0 {
                    return Err(ArithmeticError);
                }
                left.wrapping_div(right)
            }
            // `Math.floorMod`.
            IntProvider::FloorModulus(left, right) => {
                let (left, right) = (self.int_unsafe(left)?, self.int_unsafe(right)?);
                if right == 0 {
                    return Err(ArithmeticError);
                }
                let modulus = left.wrapping_rem(right);
                if modulus != 0 && (modulus ^ right) < 0 {
                    modulus + right
                } else {
                    modulus
                }
            }
            // `Math.floorDivExact`.
            IntProvider::FloorQuotient(left, right) => {
                let (left, right) = (self.int_unsafe(left)?, self.int_unsafe(right)?);
                if right == 0 {
                    return Err(ArithmeticError);
                }
                let quotient = left.checked_div(right).ok_or(ArithmeticError)?;
                if left.wrapping_rem(right) != 0 && (left ^ right) < 0 {
                    quotient - 1
                } else {
                    quotient
                }
            }
            // `Math.powExact`.
            IntProvider::Power { base, exponent } => {
                let (base, exponent) = (self.int_unsafe(base)?, self.int_unsafe(exponent)?);
                if base == 0 && exponent == 0 {
                    return Err(ArithmeticError);
                }
                let exponent = u32::try_from(exponent).map_err(|_| ArithmeticError)?;
                base.checked_pow(exponent).ok_or(ArithmeticError)?
            }
            IntProvider::FromFloat(input) => float_to_int(self.float_unsafe(input)?)?,
            IntProvider::Conditional {
                condition,
                on_true,
                on_false,
            } => {
                let branch = if self.test(condition) {
                    on_true
                } else {
                    on_false
                };
                self.int_unsafe(branch)?
            }
            IntProvider::Dispatcher { cases, default } => {
                let selected = cases
                    .iter()
                    .find(|(condition, _)| self.test(condition))
                    .map_or(&**default, |(_, value)| value);
                self.int_unsafe(selected)?
            }
            IntProvider::Weighted(entries) => {
                let selected = self.weighted(entries)?;
                self.int_unsafe(selected)?
            }
            IntProvider::Score {
                target,
                score,
                fallback,
            } => match self.score(target, score) {
                Some(value) => value,
                None => self.int_unsafe(fallback)?,
            },
            // `Number.intValue`.
            IntProvider::Storage { access, fallback } => match self.stored(access) {
                Some(NbtTag::Byte(value)) => i32::from(value),
                Some(NbtTag::Short(value)) => i32::from(value),
                Some(NbtTag::Int(value)) => value,
                Some(NbtTag::Long(value)) => value as i32,
                Some(NbtTag::Float(value)) => value as i32,
                Some(NbtTag::Double(value)) => value as i32,
                _ => self.int_unsafe(fallback)?,
            },
            IntProvider::EnvironmentAttribute(name) => {
                float_to_int(self.environment_attribute(name))?
            }
        })
    }

    #[allow(clippy::too_many_lines)]
    fn float_unsafe(&mut self, provider: &FloatProvider) -> Number<f32> {
        Ok(match provider {
            FloatProvider::Constant(value) => *value,
            FloatProvider::Uniform { min, max } => {
                let (min, max) = (self.float_unsafe(min)?, self.float_unsafe(max)?);
                self.between_float(min, max)
            }
            FloatProvider::Abs(input) => self.float_unsafe(input)?.abs(),
            FloatProvider::Negate(input) => -self.float_unsafe(input)?,
            // `Mth.ceil` gives an int.
            FloatProvider::Ceiling(input) => {
                let value = self.float_unsafe(input)?;
                let truncated = value as i32;
                (if value > truncated as f32 {
                    truncated.wrapping_add(1)
                } else {
                    truncated
                }) as f32
            }
            FloatProvider::Floor(input) => self.float_unsafe(input)?.floor(),
            // `Math.round(float)` gives an int, rounding halves up.
            FloatProvider::Round(input) => {
                (f64::from(self.float_unsafe(input)?) + 0.5).floor() as i32 as f32
            }
            FloatProvider::Truncate(input) => self.float_unsafe(input)?.trunc(),
            FloatProvider::Sine(input) => pumpkin_util::math::sin(self.float_unsafe(input)?),
            FloatProvider::Cosine(input) => pumpkin_util::math::cos(self.float_unsafe(input)?),
            FloatProvider::SquareRoot(input) => self.float_unsafe(input)?.sqrt(),
            FloatProvider::Average(inputs) => {
                let mut sum = 0.0;
                for input in inputs {
                    sum += self.float_unsafe(input)?;
                }
                sum / inputs.len() as f32
            }
            FloatProvider::Length(inputs) => {
                let mut sum = 0.0;
                for input in inputs {
                    let value = self.float_unsafe(input)?;
                    sum += value * value;
                }
                sum.sqrt()
            }
            FloatProvider::Max(inputs) => {
                let mut value = -f32::MAX;
                for input in inputs {
                    value = java_max(value, self.float_unsafe(input)?);
                }
                value
            }
            FloatProvider::Min(inputs) => {
                let mut value = f32::MAX;
                for input in inputs {
                    value = java_min(value, self.float_unsafe(input)?);
                }
                value
            }
            FloatProvider::Product(inputs) => {
                let mut value = 1.0;
                for input in inputs {
                    value *= self.float_unsafe(input)?;
                }
                value
            }
            FloatProvider::Sum(inputs) => {
                let mut value = 0.0;
                for input in inputs {
                    value += self.float_unsafe(input)?;
                }
                value
            }
            FloatProvider::Difference(left, right) => {
                self.float_unsafe(left)? - self.float_unsafe(right)?
            }
            FloatProvider::Modulus(left, right) => {
                let right = self.float_unsafe(right)?;
                if right == 0.0 {
                    f32::NAN
                } else {
                    self.float_unsafe(left)? % right
                }
            }
            FloatProvider::Quotient(left, right) => {
                self.float_unsafe(left)? / self.float_unsafe(right)?
            }
            FloatProvider::Power { base, exponent } => {
                let (base, exponent) = (self.float_unsafe(base)?, self.float_unsafe(exponent)?);
                if base == 0.0 && exponent == 0.0 {
                    f32::NAN
                } else {
                    f64::from(base).powf(f64::from(exponent)) as f32
                }
            }
            FloatProvider::FromInt(input) => self.int_unsafe(input)? as f32,
            FloatProvider::EnchantmentLevel(amount) => {
                amount.calculate(self.params.enchantment_level.unwrap_or(0))
            }
            FloatProvider::Conditional {
                condition,
                on_true,
                on_false,
            } => {
                let branch = if self.test(condition) {
                    on_true
                } else {
                    on_false
                };
                self.float_unsafe(branch)?
            }
            FloatProvider::Dispatcher { cases, default } => {
                let selected = cases
                    .iter()
                    .find(|(condition, _)| self.test(condition))
                    .map_or(&**default, |(_, value)| value);
                self.float_unsafe(selected)?
            }
            FloatProvider::Weighted(entries) => {
                let selected = self.weighted(entries)?;
                self.float_unsafe(selected)?
            }
            // `Number.floatValue`.
            FloatProvider::Storage { access, fallback } => match self.stored(access) {
                Some(NbtTag::Byte(value)) => f32::from(value),
                Some(NbtTag::Short(value)) => f32::from(value),
                Some(NbtTag::Int(value)) => value as f32,
                Some(NbtTag::Long(value)) => value as f32,
                Some(NbtTag::Float(value)) => value,
                Some(NbtTag::Double(value)) => value as f32,
                _ => self.float_unsafe(fallback)?,
            },
            FloatProvider::EnvironmentAttribute(name) => self.environment_attribute(name),
        })
    }
}

/// `Math.max(float, float)`: NaN wins.
const fn java_max(a: f32, b: f32) -> f32 {
    if a.is_nan() || b.is_nan() {
        f32::NAN
    } else {
        a.max(b)
    }
}

/// `Math.min(float, float)`: NaN wins.
const fn java_min(a: f32, b: f32) -> f32 {
    if a.is_nan() || b.is_nan() {
        f32::NAN
    } else {
        a.min(b)
    }
}
