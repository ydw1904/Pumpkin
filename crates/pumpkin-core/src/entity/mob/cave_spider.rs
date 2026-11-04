use std::sync::{Arc, Weak};

use pumpkin_data::BlockState;
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::potion::Effect;
use pumpkin_data::sound::Sound;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

use crate::entity::{
    Entity, EntityBase,
    mob::{
        Mob, MobEntity, MobMetadata,
        spawn::SpawnGroupData,
        spider::{
            initialize_spider_navigation, register_spider_goals, spider_metadata,
            update_spider_climbing,
        },
    },
};
use crate::world::World;

const VEHICLE_SEAT_HEIGHT: f64 = 0.218_75;

pub struct CaveSpiderEntity {
    pub mob_entity: MobEntity,
}

impl CaveSpiderEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mut mob_entity = MobEntity::new(entity);
        initialize_spider_navigation(&mut mob_entity);
        let mob_arc = Arc::new(Self { mob_entity });
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };
        register_spider_goals(&mob_arc.mob_entity, mob_weak);
        mob_arc
    }
}

impl Mob for CaveSpiderEntity {
    fn mob_vehicle_attachment_point(&self, vehicle: &dyn EntityBase) -> Option<Vector3<f64>> {
        super::spider::small_vehicle_attachment_point(
            &self.mob_entity,
            vehicle,
            VEHICLE_SEAT_HEIGHT,
        )
    }

    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {
        self.play_entity_sound(Sound::EntitySpiderStep, 0.15, 1.0);
    }

    // Vanilla `CaveSpider.finalizeSpawn` skips super: no jockey, effect or follow range bonus.
    fn finalize_spawn(
        &self,
        _world: &Arc<World>,
        _reason: crate::entity::mob::spawn::EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        group_data
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn mob_metadata(&self) -> Option<MobMetadata> {
        Some(spider_metadata(&self.mob_entity))
    }

    fn post_tick(&self) {
        update_spider_climbing(&self.mob_entity);
    }

    fn on_attack(&self, target: &dyn EntityBase) {
        if let Some(living) = target.get_living_entity() {
            let world = self.mob_entity.living_entity.entity.world.load();
            let duration = match world.level_info.load().difficulty {
                pumpkin_util::Difficulty::Normal => 7 * 20,
                pumpkin_util::Difficulty::Hard => 15 * 20,
                _ => 0,
            };
            if duration > 0 {
                living.add_effect_with_source(
                    Effect {
                        effect_type: &StatusEffect::POISON,
                        duration,
                        amplifier: 0,
                        ambient: false,
                        show_particles: true,
                        show_icon: true,
                        blend: false,
                    },
                    Some(self),
                );
            }
        }
    }
}
