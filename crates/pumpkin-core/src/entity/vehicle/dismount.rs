//! Vanilla `getDismountLocationForPassenger` and its `DismountHelper` checks.

use pumpkin_data::block_properties::OakTrapdoorLikeProperties;
use pumpkin_data::entity::{EntityPose, EntityType};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::{BlockDirection, BlockState};
use pumpkin_util::Hand;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

use crate::entity::{Entity, EntityBase};
use crate::world::World;

/// Vanilla `AbstractMinecart.POSE_DISMOUNT_HEIGHTS`.
const fn pose_dismount_heights(pose: EntityPose) -> &'static [i32] {
    match pose {
        EntityPose::Swimming => &[0, 1],
        _ => &[0, 1, -1],
    }
}

/// Vanilla `LivingEntity.getDismountPoses` with the `Player` override.
fn dismount_poses(passenger: &dyn EntityBase) -> &'static [EntityPose] {
    if passenger.get_player().is_some() {
        &[
            EntityPose::Standing,
            EntityPose::Crouching,
            EntityPose::Swimming,
        ]
    } else {
        &[EntityPose::Standing]
    }
}

/// Vanilla `DismountHelper.offsetsForDirection`.
fn offsets_for_direction(forward: BlockDirection) -> [(i32, i32); 8] {
    let right = forward.rotate_clockwise().to_offset();
    let left = forward.rotate_counter_clockwise().to_offset();
    let back = forward.opposite().to_offset();
    let forward = forward.to_offset();
    [
        (right.x, right.z),
        (left.x, left.z),
        (back.x + right.x, back.z + right.z),
        (back.x + left.x, back.z + left.z),
        (forward.x + right.x, forward.z + right.z),
        (forward.x + left.x, forward.z + left.z),
        (back.x, back.z),
        (forward.x, forward.z),
    ]
}

/// Vanilla `DismountHelper.isBlockFloorValid`.
fn is_block_floor_valid(block_floor_height: f64) -> bool {
    block_floor_height.is_finite() && block_floor_height < 1.0
}

/// Vanilla `Direction.fromYRot`.
fn direction_from_yaw(yaw: f32) -> BlockDirection {
    match ((yaw / 90.0) + 0.5).floor() as i32 & 3 {
        0 => BlockDirection::South,
        1 => BlockDirection::West,
        2 => BlockDirection::North,
        _ => BlockDirection::East,
    }
}

/// Vanilla `LivingEntity.getLocalBoundsForPose`, moved to `location`.
fn bounds_for_pose(passenger: &Entity, pose: EntityPose, location: Vector3<f64>) -> BoundingBox {
    BoundingBox::new_from_pos(
        location.x,
        location.y,
        location.z,
        &passenger.get_dimensions(pose),
    )
}

/// Vanilla `Entity.getCollisionHorizontalEscapeVector`.
fn collision_horizontal_escape_vector(
    collider_width: f64,
    colliding_width: f64,
    direction_degrees: f32,
) -> Vector3<f64> {
    let distance = (collider_width + colliding_width + f64::from(1.0E-5f32)) / 2.0;
    let direction_x = -(direction_degrees.to_radians()).sin();
    let direction_z = (direction_degrees.to_radians()).cos();
    let scale = direction_x.abs().max(direction_z.abs());
    Vector3::new(
        f64::from(direction_x) * distance / f64::from(scale),
        0.0,
        f64::from(direction_z) * distance / f64::from(scale),
    )
}

/// Vanilla `DismountHelper.nonClimbableShape`: climbable blocks and open trapdoors count as
/// empty.
fn non_climbable(state: &BlockState) -> bool {
    let block = state.id.to_block();
    !block.has_tag(&tag::Block::MINECRAFT_CLIMBABLE)
        && (!block.has_tag(&tag::Block::MINECRAFT_TRAPDOORS)
            || !OakTrapdoorLikeProperties::from_state_id(state.id).open)
}

/// The top of the block's collision at `pos`, relative to the block.
fn collision_top(world: &World, pos: &BlockPos, non_climbable_only: bool) -> Option<f64> {
    let state = world.get_block_state(pos);
    if non_climbable_only && !non_climbable(state) {
        return None;
    }
    state
        .get_block_collision_shapes_at(pos)
        .map(|shape| shape.max.y)
        .reduce(f64::max)
}

/// Vanilla `BlockGetter.getBlockFloorHeight`, optionally over `nonClimbableShape`.
fn block_floor_height(world: &World, pos: &BlockPos, non_climbable_only: bool) -> f64 {
    if let Some(top) = collision_top(world, pos, non_climbable_only) {
        return top;
    }
    match collision_top(world, &pos.down(), non_climbable_only) {
        Some(below) if below >= 1.0 => below - 1.0,
        _ => f64::NEG_INFINITY,
    }
}

/// Vanilla `DismountHelper.findCeilingFrom`.
fn find_ceiling_from(world: &World, pos: BlockPos, blocks: i32) -> f64 {
    for y in 0..blocks {
        let cursor = pos.up_height(y);
        let state = world.get_block_state(&cursor);
        if let Some(bottom) = state
            .get_block_collision_shapes_at(&cursor)
            .map(|shape| shape.min.y)
            .reduce(f64::min)
        {
            return f64::from(pos.0.y + y) + bottom;
        }
    }
    f64::INFINITY
}

/// Vanilla `Entity.getDismountLocationForPassenger`.
fn default_location(vehicle: &Entity) -> Vector3<f64> {
    let pos = vehicle.pos.load();
    Vector3::new(pos.x, vehicle.bounding_box.load().max.y, pos.z)
}

/// Finds where `passenger` gets off `vehicle`, setting the pose it needs there. Mirrors the
/// vanilla `getDismountLocationForPassenger` override of the vehicle's class.
pub fn dismount_location(
    world: &World,
    vehicle: &Entity,
    vehicle_is_animal: bool,
    passenger: &dyn EntityBase,
) -> Vector3<f64> {
    let entity_type = vehicle.entity_type;
    if is_minecart(entity_type) {
        minecart_location(world, vehicle, passenger)
    } else if is_boat(entity_type) {
        boat_location(world, vehicle, passenger)
    } else if is_abstract_horse(entity_type) {
        horse_location(world, vehicle, passenger)
    } else if entity_type == &EntityType::STRIDER {
        strider_location(world, vehicle, passenger)
    } else if entity_type == &EntityType::HAPPY_GHAST || !vehicle_is_animal {
        default_location(vehicle)
    } else {
        animal_location(world, vehicle, passenger)
    }
}

fn is_minecart(entity_type: &EntityType) -> bool {
    entity_type == &EntityType::MINECART
        || entity_type == &EntityType::CHEST_MINECART
        || entity_type == &EntityType::COMMAND_BLOCK_MINECART
        || entity_type == &EntityType::FURNACE_MINECART
        || entity_type == &EntityType::HOPPER_MINECART
        || entity_type == &EntityType::SPAWNER_MINECART
        || entity_type == &EntityType::TNT_MINECART
}

fn is_boat(entity_type: &EntityType) -> bool {
    entity_type.resource_name.ends_with("_boat") || entity_type.resource_name.ends_with("_raft")
}

/// The vanilla `AbstractHorse` subclasses.
fn is_abstract_horse(entity_type: &EntityType) -> bool {
    [
        &EntityType::HORSE,
        &EntityType::DONKEY,
        &EntityType::MULE,
        &EntityType::SKELETON_HORSE,
        &EntityType::ZOMBIE_HORSE,
        &EntityType::LLAMA,
        &EntityType::TRADER_LLAMA,
        &EntityType::CAMEL,
        &EntityType::CAMEL_HUSK,
    ]
    .contains(&entity_type)
}

/// Vanilla `AbstractMinecart.getDismountLocationForPassenger`.
fn minecart_location(world: &World, vehicle: &Entity, passenger: &dyn EntityBase) -> Vector3<f64> {
    // `OldMinecartBehavior.getMotionDirection`; Pumpkin doesn't track `flipped`.
    let forward = direction_from_yaw(vehicle.yaw.load()).rotate_clockwise();
    let passenger_entity = passenger.get_entity();
    let vehicle_block_pos = vehicle.block_pos.load();
    let poses = dismount_poses(passenger);
    for &pose in poses {
        let dimensions = passenger_entity.get_dimensions(pose);
        let reach = f64::from(dimensions.width.min(1.0) / 2.0);
        for &offset_y in pose_dismount_heights(pose) {
            for (offset_x, offset_z) in offsets_for_direction(forward) {
                let target = BlockPos::new(
                    vehicle_block_pos.0.x + offset_x,
                    vehicle_block_pos.0.y + offset_y,
                    vehicle_block_pos.0.z + offset_z,
                );
                let floor = block_floor_height(world, &target, true);
                if is_block_floor_valid(floor) {
                    let location = Vector3::new(
                        f64::from(target.0.x) + 0.5,
                        f64::from(target.0.y) + floor,
                        f64::from(target.0.z) + 0.5,
                    );
                    let bounds = BoundingBox::new(
                        Vector3::new(location.x - reach, location.y, location.z - reach),
                        Vector3::new(
                            location.x + reach,
                            location.y + f64::from(dimensions.height),
                            location.z + reach,
                        ),
                    );
                    if world.is_space_empty(bounds) {
                        passenger_entity.set_pose(pose);
                        return location;
                    }
                }
            }
        }
    }

    let vehicle_top = vehicle.bounding_box.load().max.y;
    let target = BlockPos::new(
        vehicle_block_pos.0.x,
        vehicle_top.floor() as i32,
        vehicle_block_pos.0.z,
    );
    for &pose in poses {
        let pose_height = f64::from(passenger_entity.get_dimensions(pose).height);
        let block_coverage = (vehicle_top - f64::from(target.0.y) + pose_height).ceil() as i32;
        if vehicle_top + pose_height <= find_ceiling_from(world, target, block_coverage) {
            passenger_entity.set_pose(pose);
            break;
        }
    }
    default_location(vehicle)
}

/// Vanilla `Animal.getDismountLocationForPassenger`.
fn animal_location(world: &World, vehicle: &Entity, passenger: &dyn EntityBase) -> Vector3<f64> {
    let forward = direction_from_yaw(vehicle.yaw.load());
    let passenger_entity = passenger.get_entity();
    let vehicle_block_pos = vehicle.block_pos.load();
    for &pose in dismount_poses(passenger) {
        for (offset_x, offset_z) in offsets_for_direction(forward) {
            let target = BlockPos::new(
                vehicle_block_pos.0.x + offset_x,
                vehicle_block_pos.0.y,
                vehicle_block_pos.0.z + offset_z,
            );
            let floor = block_floor_height(world, &target, false);
            if is_block_floor_valid(floor) {
                let location = Vector3::new(
                    f64::from(target.0.x) + 0.5,
                    f64::from(target.0.y) + floor,
                    f64::from(target.0.z) + 0.5,
                );
                if world.is_space_empty(bounds_for_pose(passenger_entity, pose, location)) {
                    passenger_entity.set_pose(pose);
                    return location;
                }
            }
        }
    }
    default_location(vehicle)
}

/// Vanilla `AbstractBoat.getDismountLocationForPassenger`.
fn boat_location(world: &World, vehicle: &Entity, passenger: &dyn EntityBase) -> Vector3<f64> {
    let passenger_entity = passenger.get_entity();
    let direction = collision_horizontal_escape_vector(
        f64::from(vehicle.entity_dimension.load().width) * std::f64::consts::SQRT_2,
        f64::from(passenger_entity.entity_dimension.load().width),
        passenger_entity.yaw.load(),
    );
    let vehicle_pos = vehicle.pos.load();
    let target_x = vehicle_pos.x + direction.x;
    let target_z = vehicle_pos.z + direction.z;
    let target = BlockPos::floored(target_x, vehicle.bounding_box.load().max.y, target_z);
    let below = target.down();
    if !world
        .get_fluid(&below)
        .has_tag(&tag::Fluid::MINECRAFT_WATER)
    {
        let mut targets = Vec::with_capacity(2);
        let target_floor = block_floor_height(world, &target, false);
        if is_block_floor_valid(target_floor) {
            targets.push(Vector3::new(
                target_x,
                f64::from(target.0.y) + target_floor,
                target_z,
            ));
        }
        let below_floor = block_floor_height(world, &below, false);
        if is_block_floor_valid(below_floor) {
            targets.push(Vector3::new(
                target_x,
                f64::from(below.0.y) + below_floor,
                target_z,
            ));
        }
        for &pose in dismount_poses(passenger) {
            for &location in &targets {
                if world.is_space_empty(bounds_for_pose(passenger_entity, pose, location)) {
                    passenger_entity.set_pose(pose);
                    return location;
                }
            }
        }
    }
    default_location(vehicle)
}

/// Vanilla `AbstractHorse.getDismountLocationForPassenger`: the passenger's main hand side
/// first, then the other side.
fn horse_location(world: &World, vehicle: &Entity, passenger: &dyn EntityBase) -> Vector3<f64> {
    let main_arm_right = passenger
        .get_player()
        .is_none_or(|player| player.config.load().main_hand == Hand::Right);
    let vehicle_width = f64::from(vehicle.entity_dimension.load().width);
    let passenger_width = f64::from(passenger.get_entity().entity_dimension.load().width);
    let yaw = vehicle.yaw.load();
    for side in [main_arm_right, !main_arm_right] {
        let direction = collision_horizontal_escape_vector(
            vehicle_width,
            passenger_width,
            yaw + if side { 90.0 } else { -90.0 },
        );
        if let Some(location) = horse_location_in_direction(world, vehicle, direction, passenger) {
            return location;
        }
    }
    vehicle.pos.load()
}

/// Vanilla `AbstractHorse.getDismountLocationInDirection`.
fn horse_location_in_direction(
    world: &World,
    vehicle: &Entity,
    direction: Vector3<f64>,
    passenger: &dyn EntityBase,
) -> Option<Vector3<f64>> {
    let passenger_entity = passenger.get_entity();
    let vehicle_pos = vehicle.pos.load();
    let vehicle_box = vehicle.bounding_box.load();
    let target_x = vehicle_pos.x + direction.x;
    let target_y = vehicle_box.min.y;
    let target_z = vehicle_pos.z + direction.z;
    let dismount_jump_limit = vehicle_box.max.y + 0.75;
    for &pose in dismount_poses(passenger) {
        let mut target = BlockPos::floored(target_x, target_y, target_z);
        loop {
            let floor = block_floor_height(world, &target, false);
            if f64::from(target.0.y) + floor > dismount_jump_limit {
                break;
            }
            if is_block_floor_valid(floor) {
                let location = Vector3::new(target_x, f64::from(target.0.y) + floor, target_z);
                if world.is_space_empty(bounds_for_pose(passenger_entity, pose, location)) {
                    passenger_entity.set_pose(pose);
                    return Some(location);
                }
            }
            target = target.up();
            if f64::from(target.0.y) >= dismount_jump_limit {
                break;
            }
        }
    }
    None
}

/// Vanilla `Strider.getDismountLocationForPassenger`.
fn strider_location(world: &World, vehicle: &Entity, passenger: &dyn EntityBase) -> Vector3<f64> {
    let passenger_entity = passenger.get_entity();
    let vehicle_width = f64::from(vehicle.entity_dimension.load().width);
    let passenger_width = f64::from(passenger_entity.entity_dimension.load().width);
    let passenger_yaw = passenger_entity.yaw.load();
    let vehicle_pos = vehicle.pos.load();
    let vehicle_box = vehicle.bounding_box.load();
    let collider_top = vehicle_box.max.y;
    let collider_bottom = vehicle_box.min.y - 0.5;

    let mut targets: Vec<BlockPos> = Vec::new();
    for yaw_offset in [0.0, -22.5, 22.5, -45.0, 45.0] {
        let direction = collision_horizontal_escape_vector(
            vehicle_width,
            passenger_width,
            passenger_yaw + yaw_offset,
        );
        let mut block_pos = BlockPos::floored(
            vehicle_pos.x + direction.x,
            collider_top,
            vehicle_pos.z + direction.z,
        );
        let mut y = collider_top;
        while y > collider_bottom {
            if !targets.contains(&block_pos) {
                targets.push(block_pos);
            }
            block_pos = block_pos.down();
            y -= 1.0;
        }
    }

    for target in targets {
        if world
            .get_fluid(&target)
            .has_tag(&tag::Fluid::MINECRAFT_LAVA)
        {
            continue;
        }
        let floor = block_floor_height(world, &target, false);
        if is_block_floor_valid(floor) {
            let location = Vector3::new(
                f64::from(target.0.x) + 0.5,
                f64::from(target.0.y) + floor,
                f64::from(target.0.z) + 0.5,
            );
            for &pose in dismount_poses(passenger) {
                if world.is_space_empty(bounds_for_pose(passenger_entity, pose, location)) {
                    passenger_entity.set_pose(pose);
                    return location;
                }
            }
        }
    }
    Vector3::new(vehicle_pos.x, vehicle_box.max.y, vehicle_pos.z)
}
