use std::sync::{Arc, atomic::Ordering};

use pumpkin_data::game_event::GameEvent;
use pumpkin_util::math::position::BlockPos;

use crate::{
    block::entities::BlockEntity, entity::EntityBase, world::World,
    world::game_event::GameEventContext,
};

pub use pumpkin_inventory::ViewerCountTracker;

pub trait ViewerCountTrackerExt {
    fn update_viewer_count<T>(&self, entity: &T, world: &Arc<World>, position: &BlockPos)
    where
        T: BlockEntity + ViewerCountListener + 'static;
}

impl ViewerCountTrackerExt for ViewerCountTracker {
    fn update_viewer_count<T>(&self, entity: &T, world: &Arc<World>, position: &BlockPos)
    where
        T: BlockEntity + ViewerCountListener + 'static,
    {
        let current = self.current.load(Ordering::Relaxed);
        let old = self.old.swap(current, Ordering::Relaxed);
        if old != current {
            let user = || self.user().and_then(|id| world.get_entity_by_id(id));
            match (old, current) {
                (n, 0) if n > 0 => {
                    entity.on_container_close(world, position, user().as_deref());
                }
                (0, n) if n > 0 => {
                    entity.on_container_open(world, position, user().as_deref());
                }
                _ => {} // Ignore
            }

            entity.on_viewer_count_update(world, position, old, current);
        }
    }
}

/// Vanilla `ContainerOpenersCounter`'s `container_open` and `container_close` game events, from
/// the entity that opened or closed the container.
pub fn emit_container_event(
    world: &World,
    event: GameEvent,
    position: &BlockPos,
    user: Option<&dyn EntityBase>,
) {
    world.emit_game_event_with_context(
        event.name(),
        position.to_centered_f64(),
        &GameEventContext {
            source_entity: user,
            affected_state: None,
        },
    );
}

pub trait ViewerCountListener: Send + Sync {
    /// The first viewer opened the container; `user` is the entity that did, if still there.
    fn on_container_open(
        &self,
        _world: &Arc<World>,
        _position: &BlockPos,
        _user: Option<&dyn EntityBase>,
    ) {
    }

    /// The last viewer closed the container; `user` is the entity that did, if still there.
    fn on_container_close(
        &self,
        _world: &Arc<World>,
        _position: &BlockPos,
        _user: Option<&dyn EntityBase>,
    ) {
    }

    fn on_viewer_count_update(
        &self,
        _world: &Arc<World>,
        _position: &BlockPos,
        _old: u16,
        _new: u16,
    ) {
    }
}
