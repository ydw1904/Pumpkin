//! Vanilla `AbstractBoat` water status and `floatBoat`.

use pumpkin_data::Block;
use pumpkin_data::fluid::{Fluid, FluidState};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_util::math::{boundingbox::BoundingBox, position::BlockPos, vector3::Vector3};

use super::BoatEntity;
use crate::entity::EntityBase;
use crate::world::World;

/// Vanilla `AbstractBoat.getDefaultGravity`.
pub(super) const DEFAULT_GRAVITY: f64 = 0.04;
/// Vertical speed under flowing water, which replaces gravity.
const UNDER_FLOWING_WATER_GRAVITY: f64 = -7.0E-4;
const DEFAULT_FRICTION: f32 = 0.9;
const UNDER_WATER_FRICTION: f32 = 0.45;
const UNDER_WATER_BUOYANCY: f32 = 0.01;
/// How far above the water's surface a boat landing on it from the air sits.
const SURFACE_OFFSET: f64 = 0.101;
const WATER_CHECK_MARGIN: f64 = 0.001;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Status {
    InWater,
    UnderWater,
    UnderFlowingWater,
    OnLand,
    InAir,
}

impl Status {
    pub(super) const fn is_under_water(self) -> bool {
        matches!(self, Self::UnderWater | Self::UnderFlowingWater)
    }
}

fn water_at(world: &World, pos: &BlockPos) -> Option<FluidState> {
    let (fluid, state) = world.get_fluid_and_fluid_state(pos);
    fluid.has_tag(&tag::Fluid::MINECRAFT_WATER).then_some(state)
}

/// Vanilla `FluidState.getHeight` of the water at `pos`.
fn water_height(world: &World, pos: &BlockPos, state: &FluidState) -> f32 {
    world.get_fluid_height(pos, &Fluid::WATER, state)
}

impl BoatEntity {
    /// Vanilla `getStatus`. Also updates the water level and land friction.
    pub(super) fn get_status(&self) -> Status {
        if let Some(status) = self.is_underwater() {
            self.water_level
                .store(self.vehicle.entity.bounding_box.load().max.y);
            return status;
        }
        if self.check_in_water() {
            return Status::InWater;
        }
        let friction = self.get_ground_friction();
        if friction > 0.0 {
            self.land_friction.store(friction);
            Status::OnLand
        } else {
            Status::InAir
        }
    }

    /// Vanilla `getWaterLevelAbove`.
    fn get_water_level_above(&self) -> f32 {
        let aabb = self.vehicle.entity.bounding_box.load();
        let min_x = aabb.min.x.floor() as i32;
        let max_x = aabb.max.x.ceil() as i32;
        let min_y = aabb.max.y.floor() as i32;
        let max_y = (aabb.max.y - self.last_yd.load()).ceil() as i32;
        let min_z = aabb.min.z.floor() as i32;
        let max_z = aabb.max.z.ceil() as i32;
        let world = self.vehicle.entity.world.load();
        'rows: for y in min_y..max_y {
            let mut block_height = 0.0f32;
            for x in min_x..max_x {
                for z in min_z..max_z {
                    let pos = BlockPos::new(x, y, z);
                    if let Some(state) = water_at(&world, &pos) {
                        block_height = block_height.max(water_height(&world, &pos, &state));
                    }
                    if block_height >= 1.0 {
                        continue 'rows;
                    }
                }
            }
            if block_height < 1.0 {
                return y as f32 + block_height;
            }
        }
        (max_y + 1) as f32
    }

    /// Vanilla `getGroundFriction`: the mean friction of the blocks under the boat.
    fn get_ground_friction(&self) -> f32 {
        let bb = self.vehicle.entity.bounding_box.load();
        let contact = BoundingBox::new(
            Vector3::new(bb.min.x, bb.min.y - WATER_CHECK_MARGIN, bb.min.z),
            Vector3::new(bb.max.x, bb.min.y, bb.max.z),
        );
        let x0 = contact.min.x.floor() as i32 - 1;
        let x1 = contact.max.x.ceil() as i32 + 1;
        let y0 = contact.min.y.floor() as i32 - 1;
        let y1 = contact.max.y.ceil() as i32 + 1;
        let z0 = contact.min.z.floor() as i32 - 1;
        let z1 = contact.max.z.ceil() as i32 + 1;
        let world = self.vehicle.entity.world.load();
        let mut friction = 0.0f32;
        let mut count = 0;
        for x in x0..x1 {
            for z in z0..z1 {
                let edges = i32::from(x == x0 || x == x1 - 1) + i32::from(z == z0 || z == z1 - 1);
                if edges == 2 {
                    continue;
                }
                for y in y0..y1 {
                    if edges > 0 && (y == y0 || y == y1 - 1) {
                        continue;
                    }
                    let pos = BlockPos::new(x, y, z);
                    let (block, state) = world.get_block_and_state(&pos);
                    if block != &Block::LILY_PAD
                        && state
                            .get_block_collision_shapes_at(&pos)
                            .any(|shape| shape.at_pos(pos).intersects(&contact))
                    {
                        friction += block.slipperiness;
                        count += 1;
                    }
                }
            }
        }
        // Vanilla divides by zero too: NaN for no blocks, which isn't above zero.
        friction / count as f32
    }

    /// Vanilla `checkInWater`. Also sets the water level.
    fn check_in_water(&self) -> bool {
        let bb = self.vehicle.entity.bounding_box.load();
        let min_x = bb.min.x.floor() as i32;
        let max_x = bb.max.x.ceil() as i32;
        let min_y = bb.min.y.floor() as i32;
        let max_y = (bb.min.y + WATER_CHECK_MARGIN).ceil() as i32;
        let min_z = bb.min.z.floor() as i32;
        let max_z = bb.max.z.ceil() as i32;
        let world = self.vehicle.entity.world.load();
        let mut in_water = false;
        let mut water_level = -f64::MAX;
        for x in min_x..max_x {
            for y in min_y..max_y {
                for z in min_z..max_z {
                    let pos = BlockPos::new(x, y, z);
                    if let Some(state) = water_at(&world, &pos) {
                        let height = y as f32 + water_height(&world, &pos, &state);
                        water_level = water_level.max(f64::from(height));
                        in_water |= bb.min.y < f64::from(height);
                    }
                }
            }
        }
        self.water_level.store(water_level);
        in_water
    }

    /// Vanilla `isUnderwater`.
    fn is_underwater(&self) -> Option<Status> {
        let aabb = self.vehicle.entity.bounding_box.load();
        let max_y = aabb.max.y + WATER_CHECK_MARGIN;
        let x0 = aabb.min.x.floor() as i32;
        let x1 = aabb.max.x.ceil() as i32;
        let y0 = aabb.max.y.floor() as i32;
        let y1 = max_y.ceil() as i32;
        let z0 = aabb.min.z.floor() as i32;
        let z1 = aabb.max.z.ceil() as i32;
        let world = self.vehicle.entity.world.load();
        let mut under_water = false;
        for x in x0..x1 {
            for y in y0..y1 {
                for z in z0..z1 {
                    let pos = BlockPos::new(x, y, z);
                    if let Some(state) = water_at(&world, &pos)
                        && max_y < f64::from(y as f32 + water_height(&world, &pos, &state))
                    {
                        if !state.is_source {
                            return Some(Status::UnderFlowingWater);
                        }
                        under_water = true;
                    }
                }
            }
        }
        under_water.then_some(Status::UnderWater)
    }

    /// Vanilla `floatBoat`: gravity, buoyancy and friction for the status, or lifting a boat
    /// that just fell into water onto its surface.
    pub(super) fn float_boat(&self, old_status: Option<Status>, status: Status) {
        let entity = &self.vehicle.entity;
        if old_status == Some(Status::InAir) && !matches!(status, Status::InAir | Status::OnLand) {
            let bounds = entity.bounding_box.load();
            self.water_level.store(bounds.max.y);
            let target_y =
                f64::from(self.get_water_level_above() - entity.height()) + SURFACE_OFFSET;
            let position = entity.pos.load();
            let offset = Vector3::new(0.0, target_y - position.y, 0.0);
            if entity.world.load().is_space_empty(bounds.shift(offset)) {
                entity.set_pos(Vector3::new(position.x, target_y, position.z));
                entity
                    .velocity
                    .store(entity.velocity.load().multiply(1.0, 0.0, 1.0));
                self.last_yd.store(0.0);
            }
            self.status.store(Some(Status::InWater));
            return;
        }
        let gravity = -self.get_gravity();
        let (vertical_speed, buoyancy, friction) = match status {
            Status::InWater => (
                gravity,
                (self.water_level.load() - entity.pos.load().y) / f64::from(entity.height()),
                DEFAULT_FRICTION,
            ),
            Status::UnderFlowingWater => (UNDER_FLOWING_WATER_GRAVITY, 0.0, DEFAULT_FRICTION),
            Status::UnderWater => (
                gravity,
                f64::from(UNDER_WATER_BUOYANCY),
                UNDER_WATER_FRICTION,
            ),
            Status::InAir => (gravity, 0.0, DEFAULT_FRICTION),
            // Vanilla halves the land friction for a player rider here, but a boat with a
            // player rider never runs this on the server.
            Status::OnLand => (gravity, 0.0, self.land_friction.load()),
        };
        let friction = f64::from(friction);
        let velocity = entity.velocity.load();
        let mut velocity = Vector3::new(
            velocity.x * friction,
            velocity.y + vertical_speed,
            velocity.z * friction,
        );
        if buoyancy > 0.0 {
            velocity.y = (velocity.y + buoyancy * (DEFAULT_GRAVITY / 0.65)) * 0.75;
        }
        entity.velocity.store(velocity);
    }
}
