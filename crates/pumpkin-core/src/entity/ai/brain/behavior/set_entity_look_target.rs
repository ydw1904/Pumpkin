use std::sync::Arc;

use super::super::BrainState;
use super::super::memory::{MemoryStatus, types};
use super::{OneShot, utils};
use crate::entity::EntityBase;

/// Vanilla `SetEntityLookTarget.create(predicate, maxDist)`.
#[must_use]
pub fn create(
    predicate: impl Fn(&BrainState, &Arc<dyn EntityBase>) -> bool + Send + Sync + 'static,
    max_dist: f32,
) -> OneShot {
    let max_dist_sqr = f64::from(max_dist * max_dist);
    OneShot::new(
        "SetEntityLookTarget",
        vec![
            (types::LOOK_TARGET.id(), MemoryStatus::ValueAbsent),
            (
                types::NEAREST_VISIBLE_LIVING_ENTITIES.id(),
                MemoryStatus::ValuePresent,
            ),
        ],
        move |tick| {
            let body = tick.mob.get_entity();
            let position = body.pos.load();
            let target = tick
                .state
                .get(types::NEAREST_VISIBLE_LIVING_ENTITIES)
                .and_then(|visible| {
                    visible.find_closest(&tick.visibility(), |target| {
                        predicate(tick.state, target)
                            && target
                                .get_entity()
                                .pos
                                .load()
                                .squared_distance_to_vec(&position)
                                <= max_dist_sqr
                            && !body.has_passenger(target.get_entity().entity_id)
                    })
                });
            let Some(target) = target else {
                return false;
            };
            utils::look_at_entity(tick.state, target);
            true
        },
    )
}
