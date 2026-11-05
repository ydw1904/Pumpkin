use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::block::blocks::fire::{FireBlockBase, fire::FireBlock};
use crate::entity::boss::ender_dragon::EnderDragonEntity;
use crate::entity::{Entity, EntityBase, living::LivingEntity};
use crate::server::Server;
use crate::world::explosion::Explosion;
use pumpkin_data::{
    Block,
    damage::DamageType,
    entity::EntityType,
    tag::{self, Taggable},
};
use pumpkin_util::math::{position::BlockPos, vector3::Vector3};
use pumpkin_world::world::BlockFlags;

pub struct EndCrystalEntity {
    entity: Entity,
    beam_target: Mutex<Option<BlockPos>>,
    show_bottom: AtomicBool,
}

impl EndCrystalEntity {
    #[must_use]
    pub fn new(mut entity: Entity) -> Self {
        entity.blocks_building = true;
        // Vanilla `EndCrystal.defineSynchedData`.
        entity.synched_data.define(
            pumpkin_data::tracked_data::end_crystal::DATA_BEAM_TARGET,
            None::<BlockPos>,
        );
        entity.synched_data.define(
            pumpkin_data::tracked_data::end_crystal::DATA_SHOW_BOTTOM,
            true,
        );
        Self {
            entity,
            beam_target: Mutex::new(None),
            show_bottom: AtomicBool::new(false),
        }
    }

    pub fn set_show_bottom(&self, show_bottom: bool) {
        self.show_bottom.store(show_bottom, Ordering::Relaxed);
        self.entity.set_synced_data(
            pumpkin_data::tracked_data::end_crystal::SHOW_BOTTOM,
            show_bottom,
        );
    }

    pub fn show_bottom(&self) -> bool {
        self.show_bottom.load(Ordering::Relaxed)
    }

    pub fn set_beam_target(&self, beam_target: Option<BlockPos>) {
        *self
            .beam_target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = beam_target;
        self.entity.set_synced_data(
            pumpkin_data::tracked_data::end_crystal::BEAM_TARGET,
            beam_target,
        );
    }

    pub fn beam_target(&self) -> Option<BlockPos> {
        *self
            .beam_target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn set_invulnerable(&self, invulnerable: bool) {
        self.entity
            .invulnerable
            .store(invulnerable, Ordering::Relaxed);
    }

    /// Vanilla `EndCrystal.onDestroyedBy`: tell the dragon fight, then the dragon. The fight's
    /// lock is released before the dragon reacts, since the dragon asks the fight for its
    /// crystals.
    fn on_destroyed_by(&self, cause: Option<&dyn EntityBase>) {
        let world = self.entity.world.load();
        let Some(fight) = &world.dragon_fight else {
            return;
        };
        let dragon_uuid = fight
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .on_crystal_destroyed(&world, self.entity.entity_uuid);
        if let Some(dragon_uuid) = dragon_uuid
            && let Some(dragon) = world
                .entities
                .load()
                .iter()
                .find(|entity| entity.get_entity().entity_uuid == dragon_uuid)
            && let Some(dragon) = dragon.cast_any().downcast_ref::<EnderDragonEntity>()
        {
            dragon.on_crystal_destroyed(self, cause);
        }
    }
}

impl EntityBase for EndCrystalEntity {
    fn get_entity(&self) -> &Entity {
        &self.entity
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        None
    }

    /// Vanilla `EndCrystal.hurtServer`: any hit but the dragon's destroys the crystal, which
    /// explodes unless an explosion destroyed it.
    fn damage_with_context(
        &self,
        _caller: &dyn EntityBase,
        _amount: f32,
        damage_type: DamageType,
        _position: Option<Vector3<f64>>,
        _source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> bool {
        if self.entity.is_invulnerable_to(&damage_type, cause) {
            return false;
        }
        if cause.is_some_and(|cause| cause.get_entity().entity_type == &EntityType::ENDER_DRAGON) {
            return false;
        }

        self.entity.remove();
        if !damage_type.has_tag(&tag::DamageType::MINECRAFT_IS_EXPLOSION) {
            let world = self.entity.world.load();
            let pos = self.entity.pos.load();
            // `damageSources().explosion(this, source.getEntity())`.
            let explosion = Explosion::new(
                6.0,
                pos,
                world.get_block_interaction(crate::world::ExplosionInteraction::Block),
            )
            .with_source(Some(self))
            .with_attacker(cause);
            world.run_explosion(&explosion, pos, 6.0);
        }
        self.on_destroyed_by(cause);
        true
    }

    /// Vanilla `EndCrystal.kill`.
    fn kill(&self, _caller: &dyn EntityBase) {
        self.on_destroyed_by(None);
        self.entity.remove();
    }

    /// Vanilla `EndCrystal.tick`: keeps a fire burning under the crystals of a dragon fight.
    fn tick(&self, caller: &dyn EntityBase, server: &Server) {
        self.entity.tick(caller, server);
        let world = self.entity.world.load();
        let pos = self.entity.block_pos.load();
        if world.dragon_fight.is_some() && world.get_block_state(&pos).is_air() {
            let fire = if FireBlockBase::get_fire_type(&world, &pos) == Block::SOUL_FIRE {
                Block::SOUL_FIRE.default_state.id
            } else {
                FireBlock.get_state_for_position(world.as_ref(), &Block::FIRE, &pos)
            };
            world.set_block_state(&pos, fire, BlockFlags::NOTIFY_ALL);
        }
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }
}
