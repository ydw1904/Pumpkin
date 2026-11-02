use crate::entity::living::{EffectSource, LivingEntity};
use pumpkin_data::data_component_impl::PotionContentsImpl;
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::potion::{Effect, Potion};

/// Vanilla `PotionContents.BASE_POTION_COLOR`.
pub const BASE_POTION_COLOR: i32 = -13_083_194;
/// Vanilla `ARGB.opaque`'s alpha bits.
pub const OPAQUE: i32 = 0xFF00_0000u32.cast_signed();

/// Utilities for reading potion contents from an `ItemStack` and applying effects.
pub struct PotionContents;

/// Source context for applying potion effects (affects scaling rules).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PotionApplicationSource {
    /// Normal application (drinking / splash)
    Normal,
    Arrow,
}

impl PotionApplicationSource {
    const fn instant_scale(self, scale: f32) -> Option<f32> {
        match self {
            Self::Arrow => None,
            Self::Normal => Some(scale),
        }
    }

    const fn duration_scale(self, scale: f32) -> f32 {
        match self {
            Self::Arrow | Self::Normal => scale,
        }
    }
}

impl PotionContents {
    /// Read effects from an `ItemStack`'s `PotionContents` data component.
    #[must_use]
    pub fn read_potion_effects(
        stack: &ItemStack,
    ) -> Vec<(&'static StatusEffect, i32, u8, bool, bool, bool)> {
        stack
            .get_data_component::<PotionContentsImpl>()
            .map(Self::effects_of)
            .unwrap_or_default()
            .into_iter()
            .map(|e| {
                (
                    e.effect_type,
                    e.duration,
                    e.amplifier,
                    e.ambient,
                    e.show_particles,
                    e.show_icon,
                )
            })
            .collect()
    }

    /// Vanilla `PotionContents.getAllEffects`: the base potion's effects, then the custom ones.
    #[must_use]
    pub fn effects_of(contents: &PotionContentsImpl) -> Vec<Effect> {
        let potion = contents
            .potion_id
            .and_then(|id| u8::try_from(id).ok())
            .and_then(Potion::from_id);
        let mut effects: Vec<Effect> = potion.map(|p| p.effects.to_vec()).unwrap_or_default();
        effects.extend(contents.custom_effects.iter().filter_map(|custom| {
            Some(Effect {
                effect_type: StatusEffect::from_minecraft_name(&custom.effect_id)?,
                duration: custom.duration,
                amplifier: custom.amplifier as u8,
                ambient: custom.ambient,
                show_particles: custom.show_particles,
                show_icon: custom.show_icon,
                blend: false,
            })
        }));
        effects
    }

    /// Vanilla `PotionContents.getColor`: the custom color, else the visible effects' colors
    /// weighted by amplifier, else `BASE_POTION_COLOR`.
    #[must_use]
    pub fn color(contents: Option<&PotionContentsImpl>) -> i32 {
        let Some(contents) = contents else {
            return BASE_POTION_COLOR;
        };
        if let Some(color) = contents.custom_color {
            return color;
        }
        let (mut red, mut green, mut blue, mut total_weight) = (0, 0, 0, 0);
        for effect in Self::effects_of(contents) {
            if effect.show_particles {
                let color = effect.effect_type.color;
                let weight = i32::from(effect.amplifier) + 1;
                red += weight * ((color >> 16) & 0xFF);
                green += weight * ((color >> 8) & 0xFF);
                blue += weight * (color & 0xFF);
                total_weight += weight;
            }
        }
        if total_weight == 0 {
            return BASE_POTION_COLOR;
        }
        OPAQUE
            | ((red / total_weight) << 16)
            | ((green / total_weight) << 8)
            | (blue / total_weight)
    }

    /// Apply instant or duration effects to a target living entity.
    pub fn apply_effects_to(
        target: &LivingEntity,
        effects: Vec<(&'static StatusEffect, i32, u8, bool, bool, bool)>,
        scale: f32,
        source: PotionApplicationSource,
        effect_source: Option<EffectSource<'_>>,
    ) {
        for (effect_type, duration, amplifier, ambient, show_particles, show_icon) in effects {
            // Like vanilla, instant effects are applied once and never added to the active
            // effects, where they would linger.
            if !target.apply_instant_effect(
                effect_type,
                amplifier,
                source.instant_scale(scale).map(f64::from),
                effect_source,
            ) {
                // Duration scaling
                let duration_scale = source.duration_scale(scale);

                let dur = if duration == -1 {
                    -1
                } else {
                    ((duration as f32) * duration_scale).max(1.0) as i32
                };
                let eff = pumpkin_data::potion::Effect {
                    effect_type,
                    duration: dur,
                    amplifier,
                    ambient,
                    show_particles,
                    show_icon,
                    blend: false,
                };
                target.add_effect(eff);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PotionApplicationSource;
    use pumpkin_data::data_component_impl::PotionDurationScaleImpl;
    use pumpkin_data::item::Item;
    use pumpkin_data::item_stack::ItemStack;

    #[test]
    fn tipped_arrow_scale_shortens_duration_without_reducing_instant_potency() {
        let tipped_arrow = ItemStack::new(1, &Item::TIPPED_ARROW);
        let scale = tipped_arrow
            .get_data_component::<PotionDurationScaleImpl>()
            .expect("tipped arrows should define a potion duration scale")
            .scale;

        assert_eq!(PotionApplicationSource::Arrow.duration_scale(scale), 0.125);
        assert_eq!(
            (160.0 * PotionApplicationSource::Arrow.duration_scale(scale)) as i32,
            20
        );
        assert_eq!(PotionApplicationSource::Arrow.instant_scale(scale), None);
    }
}
