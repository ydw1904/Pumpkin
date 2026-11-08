use pumpkin_util::math::vector3::Vector3;

use super::{air_and_water_random_pos, goal_utils, random_pos};
use crate::entity::mob::Mob;

/// Vanilla `AirRandomPos.getPosTowards`: a spot in open air, not water, towards `towards`.
#[must_use]
pub fn get_pos_towards(
    mob: &dyn Mob,
    horizontal_dist: i32,
    vertical_dist: i32,
    flying_height: i32,
    towards: Vector3<f64>,
    max_xz_radians_from_dir: f64,
) -> Option<Vector3<f64>> {
    let dir = towards - mob.get_entity().pos.load();
    let restrict = goal_utils::mob_restricted(mob, f64::from(horizontal_dist));
    let world = mob.get_entity().world.load_full();
    let mut context = air_and_water_random_pos::context(mob, &world);
    random_pos::generate_random_pos(
        || {
            air_and_water_random_pos::generate_random_pos(
                mob,
                &world,
                &mut context,
                horizontal_dist,
                vertical_dist,
                flying_height,
                dir.x,
                dir.z,
                max_xz_radians_from_dir,
                restrict,
            )
            .filter(|pos| !goal_utils::is_water(&world, pos))
        },
        |pos| f64::from(mob.get_walk_target_value(world.as_ref(), pos)),
    )
}
