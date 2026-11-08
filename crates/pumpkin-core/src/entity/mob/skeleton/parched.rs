use pumpkin_data::BlockState;
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_util::math::position::BlockPos;
use std::sync::Arc;

use crate::entity::{
    Entity, EntityBase,
    mob::{Mob, MobEntity, skeleton::SkeletonEntityBase},
    projectile::arrow::ArrowEntity,
};

pub struct ParchedSkeletonEntity {
    entity: Arc<SkeletonEntityBase>,
}

impl ParchedSkeletonEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let entity = SkeletonEntityBase::with_increased_attack_interval(entity);
        let parched = Self { entity };
        Arc::new(parched)
    }
}

impl Mob for ParchedSkeletonEntity {
    fn get_arrow(&self, arrow: ArrowEntity) -> ArrowEntity {
        super::arrow_with_effect(arrow, &StatusEffect::WEAKNESS, 600)
    }

    fn finalize_spawn(
        &self,
        _world: &std::sync::Arc<crate::world::World>,
        _reason: crate::entity::mob::spawn::EntitySpawnReason,
        group_data: Option<crate::entity::mob::spawn::SpawnGroupData>,
    ) -> Option<crate::entity::mob::spawn::SpawnGroupData> {
        self.entity.finalize_skeleton_spawn(self, group_data)
    }

    fn wants_to_pick_up(&self, stack: &ItemStack) -> bool {
        super::wants_to_pick_up(self, stack)
    }

    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {
        self.play_entity_sound(Sound::EntityParchedStep, 0.15, 1.0);
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.entity.mob_entity
    }

    fn has_audited_tick(&self) -> bool {
        true
    }
}
