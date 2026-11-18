use crate::entity::{
    Entity, EntityBase,
    mob::{Mob, MobEntity, skeleton::SkeletonEntityBase},
    player::Player,
    projectile::arrow::ArrowEntity,
    shearable::{Shearable, drop_from_shearing_loot_table, shear_by_player},
};
use pumpkin_data::BlockState;
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tracked_data;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::position::BlockPos;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

const BOGGED_SHEAR_LOOT_TABLE: &str = "minecraft:shearing/bogged";

pub struct BoggedSkeletonEntity {
    entity: Arc<SkeletonEntityBase>,
    sheared: AtomicBool,
}

impl BoggedSkeletonEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        // Vanilla `Bogged.defineSynchedData`.
        entity
            .synched_data
            .define(pumpkin_data::tracked_data::bogged::DATA_SHEARED, false);
        let entity = SkeletonEntityBase::with_increased_attack_interval(entity);
        let bogged = Self {
            entity,
            sheared: AtomicBool::new(false),
        };
        Arc::new(bogged)
    }

    #[must_use]
    pub fn is_sheared(&self) -> bool {
        self.sheared.load(Ordering::Relaxed)
    }

    pub fn set_sheared(&self, sheared: bool) {
        self.sheared.store(sheared, Ordering::Relaxed);
        self.get_entity()
            .set_synced_data(tracked_data::bogged::DATA_SHEARED, sheared);
    }
}

impl Shearable for BoggedSkeletonEntity {
    fn shear(&self, sound_category: SoundCategory, tool: &ItemStack) -> bool {
        if self.sheared.swap(true, Ordering::Relaxed) {
            return false;
        }

        let entity = self.get_entity();
        entity.world.load().play_sound(
            Sound::EntityBoggedShear,
            sound_category,
            &entity.pos.load(),
        );
        drop_from_shearing_loot_table(
            self,
            BOGGED_SHEAR_LOOT_TABLE,
            tool,
            f64::from(entity.height()),
        );
        self.set_sheared(true);
        true
    }

    fn ready_for_shearing(&self) -> bool {
        !self.is_sheared()
    }
}

impl Mob for BoggedSkeletonEntity {
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
        self.play_entity_sound(Sound::EntityBoggedStep, 0.15, 1.0);
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.entity.mob_entity
    }

    fn get_arrow(&self, arrow: ArrowEntity) -> ArrowEntity {
        super::arrow_with_effect(arrow, &StatusEffect::POISON, 100)
    }

    fn as_shearable(&self) -> Option<&dyn Shearable> {
        Some(self)
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_bool("sheared", self.is_sheared());
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.set_sheared(nbt.get_bool("sheared").unwrap_or(false));
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        if item_stack.get_item() == &Item::SHEARS && self.ready_for_shearing() {
            return shear_by_player(self, player, item_stack);
        }
        false
    }

    fn has_audited_tick(&self) -> bool {
        true
    }
}
