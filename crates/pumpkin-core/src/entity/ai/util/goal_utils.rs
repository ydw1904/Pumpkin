use pumpkin_data::entity::EntityType;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

use crate::entity::ai::pathfinder::pathfinding_context::PathfindingContext;
use crate::entity::ai::target_match::TargetMatch;
use crate::entity::mob::Mob;
use crate::world::World;

#[must_use]
pub fn mob_restricted(mob: &dyn Mob, horizontal_dist: f64) -> bool {
    let mob_entity = mob.get_mob_entity();
    if !mob_entity.has_position_target() {
        return false;
    }
    let home = mob_entity.position_target.load();
    let radius = f64::from(
        mob_entity
            .position_target_range
            .load(std::sync::atomic::Ordering::Relaxed),
    );
    let limit = radius + horizontal_dist + 1.0;
    home.to_centered_f64()
        .squared_distance_to_vec(&mob_entity.living_entity.entity.pos.load())
        < limit * limit
}

#[must_use]
pub const fn is_outside_limits(pos: &BlockPos, world: &World) -> bool {
    pos.0.y < world.dimension.min_y || pos.0.y >= world.dimension.min_y + world.dimension.height
}

#[must_use]
pub fn is_restricted(restrict: bool, mob: &dyn Mob, pos: &BlockPos) -> bool {
    restrict && !mob.get_mob_entity().is_in_position_target_range_pos(pos)
}

/// Takes the navigation lock, so callers must not already hold it.
#[must_use]
pub fn is_not_stable(mob: &dyn Mob, world: &World, pos: &BlockPos) -> bool {
    !mob.get_mob_entity()
        .navigator
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .is_stable_destination(world, pos)
}

#[must_use]
pub fn is_water(world: &World, pos: &BlockPos) -> bool {
    let (_, state_id) = world.get_block_and_state_id(pos);
    if state_id.to_state().is_waterlogged() {
        return true;
    }
    pumpkin_data::fluid::Fluid::from_state_id(state_id)
        .is_some_and(|fluid| fluid.has_tag(&tag::Fluid::MINECRAFT_WATER))
}

/// Takes the navigation lock, so callers must not already hold it.
#[must_use]
pub fn has_malus(mob: &dyn Mob, context: &mut PathfindingContext, pos: &BlockPos) -> bool {
    let path_type = context.get_land_node_type(Vector3::new(pos.0.x, pos.0.y, pos.0.z));
    let malus = mob
        .get_mob_entity()
        .navigator
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get_pathfinding_malus(path_type);
    malus != 0.0
}

#[must_use]
pub fn is_solid(world: &World, pos: &BlockPos) -> bool {
    world.get_block_state(pos).is_solid()
}

const ALERT_RANGE_Y: f64 = 10.0;

/// Vanilla `Zombie` and the classes that extend it.
const ZOMBIE_CLASS: &[&EntityType] = &[
    &EntityType::ZOMBIE,
    &EntityType::DROWNED,
    &EntityType::HUSK,
    &EntityType::ZOMBIE_VILLAGER,
    &EntityType::ZOMBIFIED_PIGLIN,
];

/// The types vanilla `getEntitiesOfClass(mob.getClass(), ..)` finds for a mob of `entity_type`.
/// Of the mobs that alert others, only the zombie's class has subclasses.
#[must_use]
pub fn same_class(entity_type: &'static EntityType) -> TargetMatch {
    if entity_type == &EntityType::ZOMBIE {
        TargetMatch::AnyOf(ZOMBIE_CLASS)
    } else {
        TargetMatch::Type(entity_type)
    }
}

/// Nearby mobs of `mob`'s class (see [`same_class`]) that are free to take over its grievance.
/// Anchored on a unit cube at the mob's feet, so a wide hitbox does not widen the call.
pub fn nearby_same_class(mob: &dyn Mob) -> Vec<std::sync::Arc<dyn crate::entity::EntityBase>> {
    use crate::entity::predicate::EntityPredicate;
    use pumpkin_data::attributes::Attributes;
    use pumpkin_util::math::boundingbox::BoundingBox;

    let mob_entity = mob.get_mob_entity();
    let entity = &mob_entity.living_entity.entity;
    let within = mob_entity
        .living_entity
        .get_attribute_value(&Attributes::FOLLOW_RANGE);

    let world = entity.world.load();
    let pos = entity.pos.load();
    let search_box =
        BoundingBox::new(pos, pos.add_raw(1.0, 1.0, 1.0)).expand(within, ALERT_RANGE_Y, within);

    let class = same_class(entity.entity_type);
    world
        .get_entities_of_types_at_box(&search_box, |entity_type| class.matches(entity_type))
        .into_iter()
        .filter(|other| {
            let other_entity = other.get_entity();
            other_entity.entity_id != entity.entity_id
                && EntityPredicate::ExceptSpectator.test(other_entity)
        })
        .collect()
}

/// How far from its body [`nearby_same_class`] reaches: the unit cube at the mob's feet grown
/// by the follow range, and by the alert height vertically.
#[must_use]
pub fn alert_range(mob_entity: &crate::entity::mob::MobEntity) -> f64 {
    use pumpkin_data::attributes::Attributes;

    mob_entity
        .living_entity
        .get_attribute_value(&Attributes::FOLLOW_RANGE)
        .max(ALERT_RANGE_Y)
        + 1.0
}
