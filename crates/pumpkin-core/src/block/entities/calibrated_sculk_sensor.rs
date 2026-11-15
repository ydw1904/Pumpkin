use std::sync::Arc;

use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::position::BlockPos;

use super::BlockEntity;
use super::sculk_sensor::SculkSensorBlockEntity;
use crate::world::World;
use crate::world::game_event::GameEventListener;

/// Vanilla `CalibratedSculkSensorBlockEntity`: a sculk sensor with the calibrated listener.
pub struct CalibratedSculkSensorBlockEntity(pub SculkSensorBlockEntity);

impl BlockEntity for CalibratedSculkSensorBlockEntity {
    fn resource_location(&self) -> &'static str {
        Self::ID
    }

    fn get_position(&self) -> BlockPos {
        self.0.position
    }

    fn from_nbt(nbt: &NbtCompound, position: BlockPos) -> Self
    where
        Self: Sized,
    {
        Self(SculkSensorBlockEntity::load(nbt, position, true))
    }

    fn write_nbt(&self, nbt: &mut NbtCompound) {
        self.0.write_nbt(nbt);
    }

    fn tick(&self, world: &Arc<World>) {
        self.0.tick(world);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn game_event_listener(&self) -> Option<&dyn GameEventListener> {
        Some(&self.0)
    }
}

impl CalibratedSculkSensorBlockEntity {
    pub const ID: &'static str = "minecraft:calibrated_sculk_sensor";

    #[must_use]
    pub fn new(position: BlockPos) -> Self {
        Self(SculkSensorBlockEntity::new_calibrated(position))
    }
}
