pub use pumpkin_data::statistic::{CustomStatistic, StatisticCategory};
use pumpkin_data::{Block, BlockId, entity::EntityType, item::Item};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use rustc_hash::FxHashMap;
use serde_json::{Map, Value};

#[derive(Default)]
pub struct Statistics {
    /// (Category ID, Statistic ID) -> Value
    pub stats: FxHashMap<(i32, i32), i32>,
}

/// The registry key of a stat's value, like `minecraft:stone` for a mined stat.
fn stat_value_key(category: StatisticCategory, stat: i32) -> Option<String> {
    let id = u16::try_from(stat).ok()?;
    let name = match category {
        StatisticCategory::Mined => Block::from_id(BlockId::new(id)?).name,
        StatisticCategory::Crafted
        | StatisticCategory::Used
        | StatisticCategory::Broken
        | StatisticCategory::PickedUp
        | StatisticCategory::Dropped => Item::from_id(id)?.registry_key,
        StatisticCategory::Killed | StatisticCategory::KilledBy => {
            EntityType::from_raw(id)?.resource_name
        }
        StatisticCategory::Custom => {
            return Some(CustomStatistic::from_i32(stat)?.registry_key().to_string());
        }
    };
    Some(format!("minecraft:{name}"))
}

/// Vanilla `Stat.buildName`, the scoreboard criterion of a stat like
/// `minecraft.mined:minecraft.stone`.
#[must_use]
pub fn criterion_name(category: StatisticCategory, stat: i32) -> Option<String> {
    Some(format!(
        "{}:{}",
        category.registry_key().replace(':', "."),
        stat_value_key(category, stat)?.replace(':', ".")
    ))
}

fn stat_value_id(category: StatisticCategory, key: &str) -> Option<i32> {
    Some(match category {
        StatisticCategory::Mined => i32::from(Block::from_name(key)?.id.as_u16()),
        StatisticCategory::Crafted
        | StatisticCategory::Used
        | StatisticCategory::Broken
        | StatisticCategory::PickedUp
        | StatisticCategory::Dropped => i32::from(Item::from_registry_key(key)?.id),
        StatisticCategory::Killed | StatisticCategory::KilledBy => {
            i32::from(EntityType::from_name(key)?.id)
        }
        StatisticCategory::Custom => CustomStatistic::from_registry_key(key)? as i32,
    })
}

impl Statistics {
    pub fn increment(&mut self, category: StatisticCategory, stat: i32, amount: i32) {
        let entry = self.stats.entry((category as i32, stat)).or_insert(0);
        *entry = entry.saturating_add(amount);
    }

    pub fn increment_custom(&mut self, stat: CustomStatistic, amount: i32) {
        self.increment(StatisticCategory::Custom, stat as i32, amount);
    }

    pub fn set(&mut self, category: StatisticCategory, stat: i32, value: i32) {
        self.stats.insert((category as i32, stat), value);
    }

    #[must_use]
    pub fn get(&self, category: StatisticCategory, stat: i32) -> i32 {
        *self.stats.get(&(category as i32, stat)).unwrap_or(&0)
    }

    /// Vanilla `ServerStatsCounter.toJson`, the contents of `players/stats/<uuid>.json`.
    #[must_use]
    pub fn to_json(&self, data_version: i32) -> Value {
        let mut by_type: Map<String, Value> = Map::new();
        for (&(category, stat), &value) in &self.stats {
            let Some(category) = StatisticCategory::from_i32(category) else {
                continue;
            };
            let Some(key) = stat_value_key(category, stat) else {
                continue;
            };
            if let Value::Object(values) = by_type
                .entry(category.registry_key())
                .or_insert_with(|| Value::Object(Map::new()))
            {
                values.insert(key, Value::from(value));
            }
        }
        let mut root = Map::new();
        root.insert("stats".to_string(), Value::Object(by_type));
        root.insert("DataVersion".to_string(), Value::from(data_version));
        Value::Object(root)
    }

    /// Vanilla `ServerStatsCounter.parse`, replacing the current stats.
    pub fn read_json(&mut self, json: &Value) {
        self.stats.clear();
        let Some(by_type) = json.get("stats").and_then(Value::as_object) else {
            return;
        };
        for (type_key, values) in by_type {
            let (Some(category), Some(values)) = (
                StatisticCategory::from_registry_key(type_key),
                values.as_object(),
            ) else {
                continue;
            };
            for (key, value) in values {
                if let (Some(stat), Some(value)) = (
                    stat_value_id(category, key),
                    value.as_i64().and_then(|v| i32::try_from(v).ok()),
                ) {
                    self.stats.insert((category as i32, stat), value);
                }
            }
        }
    }

    /// Reads the stats older Pumpkin versions kept in the player file.
    pub fn read_nbt(&mut self, nbt: &NbtCompound) {
        if let Some(stats_compound) = nbt.get_compound("Statistics") {
            for (key, tag) in &stats_compound.child_tags {
                let parts: Vec<&str> = key.split(':').collect();
                if let (NbtTag::Int(value), [cat_str, stat_str]) = (tag, parts.as_slice())
                    && let (Ok(category), Ok(stat)) =
                        (cat_str.parse::<i32>(), stat_str.parse::<i32>())
                {
                    self.stats.insert((category, stat), *value);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stats file written by a 26.3 server reads back into the same JSON.
    #[test]
    fn vanilla_stats_json_round_trips() {
        let json = serde_json::json!({
            "stats": {
                "minecraft:mined": {"minecraft:stone": 5, "minecraft:oak_log": 2},
                "minecraft:used": {"minecraft:diamond_pickaxe": 7},
                "minecraft:killed": {"minecraft:zombie": 3},
                "minecraft:custom": {"minecraft:jump": 11, "minecraft:play_time": 1200}
            },
            "DataVersion": 5023
        });
        let mut stats = Statistics::default();
        stats.read_json(&json);
        assert_eq!(
            stats.get(
                StatisticCategory::Mined,
                i32::from(Block::STONE.id.as_u16())
            ),
            5
        );
        assert_eq!(stats.to_json(5023), json);
    }

    /// Scoreboard criteria names as vanilla's `/scoreboard objectives add` accepts them.
    #[test]
    fn stat_criterion_names_match_vanilla() {
        assert_eq!(
            criterion_name(
                StatisticCategory::Mined,
                i32::from(Block::STONE.id.as_u16())
            )
            .as_deref(),
            Some("minecraft.mined:minecraft.stone")
        );
        assert_eq!(
            criterion_name(StatisticCategory::Custom, CustomStatistic::Jump as i32).as_deref(),
            Some("minecraft.custom:minecraft.jump")
        );
    }
}
