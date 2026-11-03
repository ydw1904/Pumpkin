use pumpkin_util::random::gameplay::game_random;
use std::sync::Arc;

use pumpkin_data::damage::DamageType;
use pumpkin_data::enchantment::LevelBasedValue;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::random::{RandomGenerator, RandomImpl};

use super::EnchantmentEntityEffectExt;
use crate::entity::player::Player;
use crate::entity::{Entity, EntityBase};
use crate::world::World;

/// Enchantment entity effect that damages an entity.
#[derive(Clone, Debug, PartialEq)]
pub struct DamageEntity {
    pub min_damage: LevelBasedValue,
    pub max_damage: LevelBasedValue,
    pub damage_type: Option<&'static DamageType>,
}

impl DamageEntity {
    #[must_use]
    pub const fn new(
        min_damage: LevelBasedValue,
        max_damage: LevelBasedValue,
        damage_type: Option<&'static DamageType>,
    ) -> Self {
        Self {
            min_damage,
            max_damage,
            damage_type,
        }
    }

    #[must_use]
    pub fn calculate_damage(&self, level: i32, rng: &mut impl RandomImpl) -> f32 {
        let min = self.min_damage.calculate(level);
        let max = self.max_damage.calculate(level);
        if (max - min).abs() < f32::EPSILON || min >= max {
            min
        } else {
            rng.next_f32().mul_add(max - min, min)
        }
    }

    pub fn apply(
        &self,
        world: &Arc<World>,
        enchantment_level: i32,
        owner: Option<&Arc<Player>>,
        entity: Option<&Entity>,
    ) {
        let Some(entity) = entity else {
            return;
        };
        let target_entity = world
            .get_player_by_id(entity.entity_id)
            .map(|p| p as Arc<dyn EntityBase>)
            .or_else(|| world.get_entity_by_id(entity.entity_id));
        if let Some(target) = target_entity {
            self.apply_to(
                enchantment_level,
                target.as_ref(),
                owner.map(|owner| owner.as_ref() as &dyn EntityBase),
            );
        }
    }

    /// Vanilla `DamageEntity.apply`: hurts `target` with a damage source caused directly by
    /// the enchanted item's owner.
    pub fn apply_to(
        &self,
        enchantment_level: i32,
        target: &dyn EntityBase,
        owner: Option<&dyn EntityBase>,
    ) {
        let mut rng = RandomGenerator::Gameplay(game_random());
        let damage = self.calculate_damage(enchantment_level, &mut rng);
        let damage_type = self.damage_type.copied().unwrap_or(DamageType::GENERIC);
        target.damage_with_context(target, damage, damage_type, None, owner, owner);
    }
}

impl EnchantmentEntityEffectExt for DamageEntity {
    fn apply(
        &self,
        world: &Arc<World>,
        enchantment_level: i32,
        owner: Option<&Arc<Player>>,
        entity: Option<&Entity>,
        _position: Vector3<f64>,
    ) {
        self.apply(world, enchantment_level, owner, entity);
    }
}
