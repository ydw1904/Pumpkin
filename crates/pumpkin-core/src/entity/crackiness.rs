#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CrackinessLevel {
    None,
    Low,
    Medium,
    High,
}

pub struct Crackiness {
    low: f32,
    medium: f32,
    high: f32,
}

impl Crackiness {
    pub const GOLEM: Self = Self::new(0.75, 0.5, 0.25);
    pub const WOLF_ARMOR: Self = Self::new(0.95, 0.69, 0.32);

    const fn new(low: f32, medium: f32, high: f32) -> Self {
        Self { low, medium, high }
    }

    #[must_use]
    pub fn by_fraction(&self, fraction: f32) -> CrackinessLevel {
        if fraction < self.high {
            CrackinessLevel::High
        } else if fraction < self.medium {
            CrackinessLevel::Medium
        } else if fraction < self.low {
            CrackinessLevel::Low
        } else {
            CrackinessLevel::None
        }
    }

    #[must_use]
    pub fn by_damage(&self, damage: i32, max_damage: i32) -> CrackinessLevel {
        self.by_fraction((max_damage - damage) as f32 / max_damage as f32)
    }

    /// Vanilla `byDamage(ItemStack)`: an item that can't take damage isn't cracked.
    #[must_use]
    pub fn by_item_damage(&self, item: &pumpkin_data::item_stack::ItemStack) -> CrackinessLevel {
        match item.get_max_damage() {
            Some(max_damage) if max_damage > 0 && !item.is_unbreakable() => {
                self.by_damage(item.get_damage(), max_damage)
            }
            _ => CrackinessLevel::None,
        }
    }
}
