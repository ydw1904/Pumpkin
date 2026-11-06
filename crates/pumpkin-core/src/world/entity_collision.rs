use std::any::Any;

use pumpkin_data::entity::EntityType;
use pumpkin_util::math::boundingbox::BoundingBox;

use super::{World, entity_islands};
use crate::entity::{
    Entity, EntityBase, ageable::AgeableMob, mob::shulker::ShulkerEntity,
    passive::happy_ghast::HappyGhastEntity, vehicle::boat::BoatEntity,
    vehicle::minecart::MinecartEntity,
};

const COLLISION_EPSILON: f64 = 1.0e-7;

/// Vanilla `Entity.canBeCollidedWith`, with `other` the moving entity, if any.
fn can_be_collided_with(entity: &dyn EntityBase, other: Option<&Entity>) -> bool {
    let any = entity as &dyn Any;
    if any.is::<BoatEntity>() {
        return true;
    }
    if any.is::<ShulkerEntity>() {
        return entity
            .get_living_entity()
            .is_some_and(|living| living.health.load() > 0.0);
    }
    any.downcast_ref::<HappyGhastEntity>().is_some_and(|ghast| {
        !ghast.is_baby()
            && ghast.mob_entity.living_entity.health.load() > 0.0
            && ((ghast.get_entity().has_passengers()
                && other.is_some_and(|other| other.entity_type == &EntityType::HAPPY_GHAST))
                || ghast.is_on_still_timeout())
    })
}

/// Boats and minecarts also collide with pushable entities (`AbstractBoat.canVehicleCollide`).
fn collides_with_pushables(source: &dyn EntityBase) -> bool {
    let any = source as &dyn Any;
    any.is::<BoatEntity>() || any.is::<MinecartEntity>()
}

/// Vanilla `getEntityCollisions` predicate: `CAN_BE_COLLIDED_WITH` without a source,
/// `NO_SPECTATORS.and(source::canCollideWith)` with one.
fn collides(source: Option<&dyn EntityBase>, entity: &dyn EntityBase) -> bool {
    let base = entity.get_entity();
    if entity.is_spectator() || base.is_removed() {
        return false;
    }
    let Some(source) = source else {
        return can_be_collided_with(entity, None);
    };
    let source_base = source.get_entity();
    if source_base.entity_id == base.entity_id {
        return false;
    }
    (can_be_collided_with(entity, Some(source_base))
        || (collides_with_pushables(source) && entity.is_pushable()))
        && !source_base.is_passenger_of_same_vehicle(base)
}

impl World {
    /// Vanilla `EntityGetter.getEntityCollisions`: the boxes of the entities in `area` that
    /// `source` collides with. Players only collide with nothing here, as the client moves them.
    pub fn get_entity_collisions(
        &self,
        source: Option<&dyn EntityBase>,
        area: BoundingBox,
    ) -> Vec<BoundingBox> {
        if area.get_average_side_length() < COLLISION_EPSILON {
            return Vec::new();
        }
        let area = area.expand_all(COLLISION_EPSILON);
        let mut boxes: Vec<BoundingBox> = self
            .entity_index
            .query(self.entities.load_full(), area)
            .inspect(|entity| entity_islands::check_reached(entity.get_entity().entity_id))
            .filter(|entity| collides(source, entity.as_ref()))
            .map(|entity| entity.get_entity().bounding_box.load())
            .collect();
        // Only boats and minecarts can collide with a player, which is pushable.
        if source.is_some_and(collides_with_pushables) {
            boxes.extend(
                self.get_players_at_box(&area)
                    .iter()
                    .filter(|player| collides(source, player.as_ref()))
                    .map(|player| player.get_entity().bounding_box.load()),
            );
        }
        boxes
    }

    /// Vanilla `noEntityCollision`.
    pub(crate) fn no_entity_collision(
        &self,
        source: Option<&dyn EntityBase>,
        aabb: &BoundingBox,
    ) -> bool {
        self.get_entity_collisions(source, *aabb).is_empty()
    }

    /// Vanilla `noBorderCollision`: only checked while `source` is close to the border.
    fn no_border_collision(&self, source: &Entity, aabb: &BoundingBox) -> bool {
        let border = self
            .worldborder
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        !border.is_inside_close_to_border(source, aabb) || border.collision_shape_contains(aabb)
    }

    /// Vanilla `CollisionGetter.noCollision(source, aabb)`: no block, entity or world border
    /// collision for `source` in `aabb`.
    pub fn no_collision(&self, source: &dyn EntityBase, aabb: &BoundingBox) -> bool {
        self.get_block_collisions(*aabb, source).0.is_empty()
            && self.no_entity_collision(Some(source), aabb)
            && self.no_border_collision(source.get_entity(), aabb)
    }
}
