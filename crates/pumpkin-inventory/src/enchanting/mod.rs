//! Enchanting module.
//!
//! This module handles enchanting mechanics including:
//! - [`EnchantingTableScreenHandler`] - Screen handler for enchanting tables

pub mod enchanting_screen_handler;

use pumpkin_data::Enchantment;
use pumpkin_data::data_component_impl::EnchantableImpl;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_util::random::RandomImpl;

/// `EnchantmentHelper.selectEnchantment`: the enchantments `cost` levels buy for `item`, picked
/// by weight from `source`.
pub fn select_enchantment(
    random: &mut impl RandomImpl,
    item: &ItemStack,
    mut cost: i32,
    source: impl Iterator<Item = &'static Enchantment>,
) -> Vec<(&'static Enchantment, i32)> {
    let Some(enchantability) = item
        .get_data_component::<EnchantableImpl>()
        .map(|e| e.value)
    else {
        return Vec::new();
    };
    cost += 1
        + random.next_bounded_i32(enchantability / 4 + 1)
        + random.next_bounded_i32(enchantability / 4 + 1);
    let span = (random.next_f32() + random.next_f32() - 1.0) * 0.15;
    cost = ((cost as f32 + cost as f32 * span).round() as i32).max(1);

    // `getAvailableEnchantmentResults`: books take every enchantment of the source.
    let is_book = item.item == &Item::BOOK;
    let mut available = Vec::new();
    for enchant in source {
        if is_book || enchant.is_primary_item(item.item) {
            for l in (1..=enchant.max_level).rev() {
                if cost >= enchant.min_cost.calculate(l) && cost <= enchant.max_cost.calculate(l) {
                    available.push((enchant, l));
                    break;
                }
            }
        }
    }

    let mut result: Vec<(&'static Enchantment, i32)> = Vec::new();
    if !available.is_empty() {
        result.extend(weighted_pick(random, &available));
        while random.next_bounded_i32(50) <= cost {
            if let Some((last, _)) = result.last() {
                available.retain(|(e, _)| e.are_compatible(last));
            }
            if available.is_empty() {
                break;
            }
            result.extend(weighted_pick(random, &available));
            cost /= 2;
        }
    }
    result
}

/// `WeightedRandom.getRandomItem`.
fn weighted_pick(
    random: &mut impl RandomImpl,
    available: &[(&'static Enchantment, i32)],
) -> Option<(&'static Enchantment, i32)> {
    let total_weight: i32 = available.iter().map(|(e, _)| e.weight).sum();
    if total_weight <= 0 {
        return None;
    }
    let mut weight = random.next_bounded_i32(total_weight);
    for (e, l) in available {
        weight -= e.weight;
        if weight < 0 {
            return Some((*e, *l));
        }
    }
    None
}
