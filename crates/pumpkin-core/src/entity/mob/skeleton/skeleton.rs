use pumpkin_data::BlockState;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::tracked_data;
use pumpkin_data::world::WorldEvent;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::position::BlockPos;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::entity::{
    Entity, EntityBase,
    mob::{
        Mob, MobEntity, conversion_tracker::ConversionTracker, equipment::RegionalDifficulty,
        skeleton::SkeletonEntityBase,
    },
};
use crate::world::World;

pub struct SkeletonEntity {
    entity: Arc<SkeletonEntityBase>,
    /// Vanilla `Skeleton.freezingTracker`: seven seconds in powder snow start the conversion
    /// into a stray.
    freezing_tracker: ConversionTracker,
}

impl SkeletonEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let entity = SkeletonEntityBase::new(entity);
        let skeleton = Self {
            entity,
            freezing_tracker: ConversionTracker::new(
                tracked_data::skeleton::DATA_STRAY_CONVERSION_ID,
                "FreezingTime",
                140,
                "StrayConversionTime",
                300,
            ),
        };
        Arc::new(skeleton)
    }

    fn is_in_powder_snow(&self) -> bool {
        self.entity
            .mob_entity
            .living_entity
            .entity
            .is_in_powder_snow
            .load(Ordering::Relaxed)
    }
}

impl Mob for SkeletonEntity {
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
        self.play_entity_sound(Sound::EntitySkeletonStep, 0.15, 1.0);
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.entity.mob_entity
    }

    // Vanilla `Skeleton.tick` runs the freezing tracker before the rest of the tick.
    fn mob_tick(&self, _caller: &dyn EntityBase) {
        let mob_entity = &self.entity.mob_entity;
        self.freezing_tracker
            .tick(mob_entity, self.is_in_powder_snow(), || {
                let _ = mob_entity.convert_to(&EntityType::STRAY, |stray| {
                    let stray_entity = stray.get_entity();
                    if !stray_entity.is_silent() {
                        stray_entity.world.load().sync_world_event(
                            WorldEvent::SoundSkeletonToStray,
                            stray_entity.block_pos.load(),
                            0,
                        );
                    }
                });
            });
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.freezing_tracker
            .write_nbt(nbt, self.is_in_powder_snow());
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.freezing_tracker.read_nbt(&self.entity.mob_entity, nbt);
    }

    fn populate_default_equipment_slots(
        &self,
        world: &Arc<World>,
        difficulty: &RegionalDifficulty,
    ) {
        self.entity
            .populate_default_equipment_slots(world, difficulty);
    }

    fn populate_default_equipment_enchantments(&self, difficulty: &RegionalDifficulty) {
        self.entity
            .populate_default_equipment_enchantments(difficulty);
    }
}
