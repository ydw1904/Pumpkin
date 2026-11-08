//! Vanilla `BreezeUtil`, plus `Breeze.withinInnerCircleRange`.

use pumpkin_data::attributes::Attributes;
use pumpkin_util::math::{cos, lerp, sin, vector3::Vector3};
use pumpkin_util::random::gameplay::GameRandom;
use pumpkin_world::clip::{ClipBlock, ClipContext, ClipFluid};

use crate::entity::Entity;
use crate::entity::mob::Mob;

/// Vanilla `BreezeUtil.MAX_LINE_OF_SIGHT_TEST_RANGE`.
const MAX_LINE_OF_SIGHT_TEST_RANGE: f64 = 50.0;
/// Vanilla `BreezeAi.JUMP_CIRCLE_INNER_RADIUS`.
pub(super) const JUMP_CIRCLE_INNER_RADIUS: f32 = 4.0;
/// Vanilla `BreezeAi.JUMP_CIRCLE_MIDDLE_RADIUS`.
pub(super) const JUMP_CIRCLE_MIDDLE_RADIUS: f32 = 8.0;
/// The vertical half-height of `Breeze.withinInnerCircleRange`.
const INNER_CIRCLE_HEIGHT: f64 = 10.0;
/// The spread of `BreezeUtil.randomPointBehindTarget`, in degrees.
const SPREAD_DEGREES: f32 = 90.0;

/// Vanilla `BreezeUtil.randomPointBehindTarget`: 4 to 8 blocks behind where `enemy` looks.
pub(super) fn random_point_behind_target(enemy: &Entity, random: GameRandom) -> Vector3<f64> {
    let view_angle =
        enemy.head_yaw.load() + 180.0 + random.next_gaussian() as f32 * SPREAD_DEGREES / 2.0;
    let r = lerp(
        random.random::<f32>(),
        JUMP_CIRCLE_INNER_RADIUS,
        JUMP_CIRCLE_MIDDLE_RADIUS,
    );
    // `Vec3.directionFromRotation(0.0F, viewAngle)`.
    let radians = view_angle.to_radians();
    let direction = Vector3::new(f64::from(-sin(radians)), 0.0, f64::from(cos(radians)));
    enemy.pos.load() + direction * f64::from(r)
}

/// Vanilla `BreezeUtil.hasLineOfSight`: an unobstructed collider ray from the breeze's feet.
pub(super) fn has_line_of_sight(breeze: &dyn Mob, target: Vector3<f64>) -> bool {
    let entity = breeze.get_entity();
    let from = entity.pos.load();
    let max_range = MAX_LINE_OF_SIGHT_TEST_RANGE.max(
        breeze
            .get_mob_entity()
            .living_entity
            .get_attribute_value(&Attributes::FOLLOW_RANGE),
    );
    if target.sub(&from).length() > max_range {
        return false;
    }
    entity
        .world
        .load()
        .clip(&ClipContext::new(
            from,
            target,
            ClipBlock::Collider,
            ClipFluid::None,
        ))
        .is_miss
}

/// Vanilla `Breeze.withinInnerCircleRange`.
pub(super) fn within_inner_circle_range(breeze: &Entity, target: Vector3<f64>) -> bool {
    let center = breeze.block_pos.load().to_centered_f64();
    let (dx, dz) = (target.x - center.x, target.z - center.z);
    dx * dx + dz * dz < f64::from(JUMP_CIRCLE_INNER_RADIUS * JUMP_CIRCLE_INNER_RADIUS)
        && (target.y - center.y).abs() < INNER_CIRCLE_HEIGHT
}
