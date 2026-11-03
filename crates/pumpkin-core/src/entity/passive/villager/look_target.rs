use pumpkin_data::attributes::Attributes;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::random::gameplay::game_random;

use super::VillagerEntity;
use crate::entity::EntityBase;
use crate::entity::mob::Mob;
use crate::world::World;

/// The `LookAtTargetSink(45, 90)` of vanilla's villager core package.
const LOOK_AT_TARGET_SINK_MIN_DURATION: i32 = 45;
const LOOK_AT_TARGET_SINK_MAX_DURATION: i32 = 90;

/// The `PositionTracker` of vanilla's `LOOK_TARGET` memory.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum LookTarget {
    /// `BlockPosTracker` or a fixed position.
    Pos(Vector3<f64>),
    /// `EntityTracker` that tracks the eye height.
    Entity(i32),
}

impl VillagerEntity {
    pub(super) fn look_target(&self) -> Option<LookTarget> {
        *self
            .look_target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn set_look_target(&self, target: Option<LookTarget>) {
        *self
            .look_target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = target;
    }

    /// Sets the `LOOK_TARGET` memory to a position, which the core `LookAtTargetSink` turns
    /// the head to.
    pub(super) fn look_at_position(&self, target: Vector3<f64>) {
        self.set_look_target(Some(LookTarget::Pos(target)));
    }

    /// Vanilla `BehaviorUtils.lookAtEntity`.
    pub(super) fn look_at_entity(&self, entity_id: i32) {
        self.set_look_target(Some(LookTarget::Entity(entity_id)));
    }

    pub(super) fn erase_look_target(&self) {
        self.set_look_target(None);
    }

    /// Vanilla `EntityTracker.isVisibleBy`: a living entity has to be alive and among the
    /// villager's visible living entities. Positions are always visible.
    fn look_target_position(&self, world: &World, target: LookTarget) -> Option<Vector3<f64>> {
        let id = match target {
            LookTarget::Pos(pos) => return Some(pos),
            LookTarget::Entity(id) => id,
        };
        let entity = world.get_entity_by_id(id)?;
        if let Some(living) = entity.get_living_entity() {
            let range = self
                .mob_entity
                .living_entity
                .get_attribute_value(&Attributes::FOLLOW_RANGE);
            let in_range = self
                .get_entity()
                .bounding_box
                .load()
                .expand(range, range, range)
                .intersects(&entity.get_entity().bounding_box.load());
            if !living.is_alive() || !in_range || !self.has_line_of_sight(entity.get_entity()) {
                return None;
            }
        }
        Some(entity.get_entity().get_eye_pos())
    }

    /// The start check of vanilla `LookAtTargetSink`, which runs before the other behaviors
    /// start.
    pub(super) fn start_look_at_target_sink(&self, game_time: i64) {
        let mut end = self
            .look_at_target_sink_end
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if end.is_none() && self.look_target().is_some() {
            *end = Some(
                game_time
                    + i64::from(game_random().random_range(
                        LOOK_AT_TARGET_SINK_MIN_DURATION..=LOOK_AT_TARGET_SINK_MAX_DURATION,
                    )),
            );
        }
    }

    /// Vanilla `Behavior.tickOrStop` for `LookAtTargetSink`: turns the head to the look target
    /// until it times out or the target can't be seen, then erases the memory.
    pub(super) fn tick_look_at_target_sink(&self, world: &World, game_time: i64) {
        let mut end = self
            .look_at_target_sink_end
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(end_time) = *end else {
            return;
        };
        let position = self
            .look_target()
            .and_then(|target| self.look_target_position(world, target));
        match position {
            Some(position) if game_time <= end_time => {
                self.mob_entity
                    .look_control
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .look_at_position(self, position);
            }
            _ => {
                *end = None;
                self.erase_look_target();
            }
        }
    }
}
