//! Vanilla `BlockGetter`'s block traversal along a moving box.

use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use rustc_hash::FxHashSet;

/// Vanilla `BlockGetter.forEachBlockIntersectedBetween`.
///
/// Visits each block the box touches on its way from `from` to `to` (where it is
/// `aabb_at_target`) with the travel step it was reached at, until `visitor` returns false.
pub fn for_each_block_intersected_between(
    from: Vector3<f64>,
    to: Vector3<f64>,
    aabb_at_target: &BoundingBox,
    mut visitor: impl FnMut(BlockPos, i32) -> bool,
) -> bool {
    let travel = to - from;
    if travel.length_squared() < f64::from(1.0E-5f32 * 1.0E-5f32) {
        return between_closed(aabb_at_target).all(|pos| visitor(pos, 0));
    }

    let mut visited = FxHashSet::default();
    let start = aabb_at_target.shift(travel * -1.0);
    for pos in between_corners_in_direction(
        BlockPos::floored_v(start.min),
        BlockPos::floored_v(start.max),
        travel,
    ) {
        if !visitor(pos, 0) {
            return false;
        }
        visited.insert(pos);
    }

    let Some(iterations) =
        add_collisions_along_travel(&mut visited, travel, aabb_at_target, &mut visitor)
    else {
        return false;
    };

    between_corners_in_direction(
        BlockPos::floored_v(aabb_at_target.min),
        BlockPos::floored_v(aabb_at_target.max),
        travel,
    )
    .all(|pos| !visited.insert(pos) || visitor(pos, iterations + 1))
}

/// Vanilla `BlockPos.betweenClosed(AABB)`.
fn between_closed(aabb: &BoundingBox) -> impl Iterator<Item = BlockPos> {
    BlockPos::iterate(BlockPos::floored_v(aabb.min), BlockPos::floored_v(aabb.max))
}

/// Vanilla `BlockPos.betweenCornersInDirection`: the blocks between two corners, starting at
/// the corner `direction` comes from and stepping along `Direction.axisStepOrder(direction)`.
fn between_corners_in_direction(
    first: BlockPos,
    second: BlockPos,
    direction: Vector3<f64>,
) -> impl Iterator<Item = BlockPos> {
    let min = BlockPos::new(
        first.0.x.min(second.0.x),
        first.0.y.min(second.0.y),
        first.0.z.min(second.0.z),
    );
    let max = BlockPos::new(
        first.0.x.max(second.0.x),
        first.0.y.max(second.0.y),
        first.0.z.max(second.0.z),
    );
    let start = |positive: bool, min: i32, max: i32| if positive { min } else { max };
    let start = Vector3::new(
        start(direction.x >= 0.0, min.0.x, max.0.x),
        start(direction.y >= 0.0, min.0.y, max.0.y),
        start(direction.z >= 0.0, min.0.z, max.0.z),
    );
    let step = |value: f64| if value >= 0.0 { 1 } else { -1 };
    let unit = |axis: usize, value: f64| {
        let mut v = Vector3::new(0, 0, 0);
        match axis {
            0 => v.x = step(value),
            1 => v.y = step(value),
            _ => v.z = step(value),
        }
        v
    };
    // Vanilla `Direction.axisStepOrder`: Y first, then the smaller horizontal axis last.
    let axes = if direction.x.abs() < direction.z.abs() {
        [1, 2, 0]
    } else {
        [1, 0, 2]
    };
    let component = |axis: usize, v: Vector3<f64>| match axis {
        0 => v.x,
        1 => v.y,
        _ => v.z,
    };
    let size = |axis: usize| match axis {
        0 => max.0.x - min.0.x,
        1 => max.0.y - min.0.y,
        _ => max.0.z - min.0.z,
    };
    let [first_axis, second_axis, third_axis] = axes;
    let (first_dir, second_dir, third_dir) = (
        unit(first_axis, component(first_axis, direction)),
        unit(second_axis, component(second_axis, direction)),
        unit(third_axis, component(third_axis, direction)),
    );
    let (first_max, second_max, third_max) =
        (size(first_axis), size(second_axis), size(third_axis));
    (0..=first_max).flat_map(move |i| {
        (0..=second_max).flat_map(move |j| {
            (0..=third_max)
                .map(move |k| BlockPos(start + first_dir * i + second_dir * j + third_dir * k))
        })
    })
}

const fn sign(value: f64) -> i32 {
    if value > 0.0 {
        1
    } else if value < 0.0 {
        -1
    } else {
        0
    }
}

/// Vanilla `BlockGetter.addCollisionsAlongTravel`: steps the box's leading corner through the
/// blocks between the start and the target box. Returns the step count, or `None` once
/// `visitor` stops the traversal.
fn add_collisions_along_travel(
    visited: &mut FxHashSet<BlockPos>,
    delta: Vector3<f64>,
    aabb_at_target: &BoundingBox,
    visitor: &mut impl FnMut(BlockPos, i32) -> bool,
) -> Option<i32> {
    let size = aabb_at_target.max - aabb_at_target.min;
    let corner = furthest_corner(delta);
    let center = aabb_at_target.get_center();
    let to_corner = Vector3::new(
        center.x + size.x * 0.5 * f64::from(corner.x),
        center.y + size.y * 0.5 * f64::from(corner.y),
        center.z + size.z * 0.5 * f64::from(corner.z),
    );
    let from_corner = to_corner - delta;
    let mut block = BlockPos::floored_v(from_corner).0;
    let (sign_x, sign_y, sign_z) = (sign(delta.x), sign(delta.y), sign(delta.z));
    let t_delta = |sign: i32, delta: f64| {
        if sign == 0 {
            f64::MAX
        } else {
            f64::from(sign) / delta
        }
    };
    let (t_delta_x, t_delta_y, t_delta_z) = (
        t_delta(sign_x, delta.x),
        t_delta(sign_y, delta.y),
        t_delta(sign_z, delta.z),
    );
    let t_start = |sign: i32, t_delta: f64, coord: f64| {
        let frac = coord - coord.floor();
        t_delta * if sign > 0 { 1.0 - frac } else { frac }
    };
    let mut t_x = t_start(sign_x, t_delta_x, from_corner.x);
    let mut t_y = t_start(sign_y, t_delta_y, from_corner.y);
    let mut t_z = t_start(sign_z, t_delta_z, from_corner.z);
    let epsilon = f64::from(1.0E-5f32);
    let mut iterations = 0;

    while t_x <= 1.0 || t_y <= 1.0 || t_z <= 1.0 {
        if t_x < t_y {
            if t_x < t_z {
                block.x += sign_x;
                t_x += t_delta_x;
            } else {
                block.z += sign_z;
                t_z += t_delta_z;
            }
        } else if t_y < t_z {
            block.y += sign_y;
            t_y += t_delta_y;
        } else {
            block.z += sign_z;
            t_z += t_delta_z;
        }

        let block_pos = BlockPos(block);
        let Some(hit) = BoundingBox::from_block(&block_pos).clip(from_corner, to_corner) else {
            continue;
        };
        iterations += 1;
        let (bx, by, bz) = (f64::from(block.x), f64::from(block.y), f64::from(block.z));
        let opposite = BlockPos::floored_v(Vector3::new(
            hit.x.clamp(bx + epsilon, bx + 1.0 - epsilon) - size.x * f64::from(corner.x),
            hit.y.clamp(by + epsilon, by + 1.0 - epsilon) - size.y * f64::from(corner.y),
            hit.z.clamp(bz + epsilon, bz + 1.0 - epsilon) - size.z * f64::from(corner.z),
        ));
        for pos in between_corners_in_direction(block_pos, opposite, delta) {
            if visited.insert(pos) && !visitor(pos, iterations) {
                return None;
            }
        }
    }
    Some(iterations)
}

/// Vanilla `BlockGetter.getFurthestCorner`.
fn furthest_corner(direction: Vector3<f64>) -> Vector3<i32> {
    let x_dot = direction.x.abs();
    let y_dot = direction.y.abs();
    let z_dot = direction.z.abs();
    let x_sign = if direction.x >= 0.0 { 1 } else { -1 };
    let y_sign = if direction.y >= 0.0 { 1 } else { -1 };
    let z_sign = if direction.z >= 0.0 { 1 } else { -1 };
    if x_dot <= y_dot && x_dot <= z_dot {
        Vector3::new(-x_sign, -z_sign, y_sign)
    } else if y_dot <= z_dot {
        Vector3::new(z_sign, -y_sign, -x_sign)
    } else {
        Vector3::new(-y_sign, x_sign, -z_sign)
    }
}
