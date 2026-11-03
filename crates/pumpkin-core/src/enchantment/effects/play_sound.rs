use std::sync::Arc;

use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_util::math::float_provider::FloatProvider;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::random::RandomGenerator;
use pumpkin_util::random::gameplay::game_random;

use super::EnchantmentEntityEffectExt;
use crate::entity::Entity;
use crate::entity::player::Player;
use crate::world::World;

/// Vanilla `PlaySoundEffect`.
#[derive(Clone, Debug, PartialEq)]
pub struct PlaySound {
    pub sounds: &'static [&'static str],
    pub volume: FloatProvider,
    pub pitch: FloatProvider,
}

impl PlaySound {
    #[must_use]
    pub const fn new(
        sounds: &'static [&'static str],
        volume: FloatProvider,
        pitch: FloatProvider,
    ) -> Self {
        Self {
            sounds,
            volume,
            pitch,
        }
    }
}

impl EnchantmentEntityEffectExt for PlaySound {
    fn apply(
        &self,
        world: &Arc<World>,
        enchantment_level: i32,
        _owner: Option<&Arc<Player>>,
        entity: Option<&Entity>,
        position: Vector3<f64>,
    ) {
        if entity.is_some_and(Entity::is_silent) {
            return;
        }
        // One sound per enchantment level, the last one for higher levels.
        let index = usize::try_from(enchantment_level - 1)
            .unwrap_or(0)
            .min(self.sounds.len().saturating_sub(1));
        let Some(sound) = self
            .sounds
            .get(index)
            .and_then(|name| Sound::from_name(name.strip_prefix("minecraft:").unwrap_or(name)))
        else {
            return;
        };
        let category = entity
            .and_then(|entity| world.get_entity_by_id(entity.entity_id))
            .map_or(SoundCategory::Neutral, |entity| entity.sound_source());
        let mut random = RandomGenerator::Gameplay(game_random());
        let volume = self.volume.get(&mut random);
        let pitch = self.pitch.get(&mut random);
        world.play_sound_fine(sound, category, &position, volume, pitch);
    }
}
