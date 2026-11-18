use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use dashmap::DashMap;
use pumpkin_data::{BlockStateId, game_event::GameEvent};
use pumpkin_util::math::{
    boundingbox::BoundingBox,
    position::{BlockPos, chunk_section_from_pos},
    vector2::Vector2,
    vector3::Vector3,
};
use pumpkin_world::generation::section_coords::section_to_block;

use super::{World, entity_islands};
use crate::block::entities::BlockEntity;
use crate::entity::EntityBase;

/// Vanilla `GameEvent.Context`.
pub struct GameEventContext<'a> {
    pub source_entity: Option<&'a dyn EntityBase>,
    pub affected_state: Option<BlockStateId>,
}

pub trait GameEventListener {
    fn listener_position(&self) -> Vector3<f64>;

    fn listener_radius(&self) -> i32;

    fn handle_game_event(
        &self,
        world: &World,
        event: GameEvent,
        context: &GameEventContext<'_>,
        source_position: Vector3<f64>,
    ) -> bool;

    /// Vanilla `DeliveryMode.BY_DISTANCE`: handled after the other listeners, closest first.
    fn delivered_by_distance(&self) -> bool {
        false
    }
}

/// The loaded block entities that listen to game events, by chunk, in registration order.
/// Vanilla keeps them in a `EuclideanGameEventListenerRegistry` per chunk section.
#[derive(Default)]
pub struct BlockListenerRegistry {
    count: AtomicUsize,
    chunks: DashMap<Vector2<i32>, Vec<BlockPos>>,
}

impl BlockListenerRegistry {
    /// Registers the block entity now at its position, or drops the registration when it
    /// doesn't listen.
    pub fn update(&self, block_entity: &dyn BlockEntity) {
        let pos = block_entity.get_position();
        if block_entity.game_event_listener().is_none() {
            self.remove(&pos);
            return;
        }
        let mut positions = self.chunks.entry(pos.chunk_position()).or_default();
        if !positions.contains(&pos) {
            positions.push(pos);
            self.count.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn remove(&self, pos: &BlockPos) {
        let chunk = pos.chunk_position();
        let removed = self
            .chunks
            .get_mut(&chunk)
            .and_then(|mut positions| {
                let index = positions.iter().position(|p| p == pos)?;
                positions.remove(index);
                Some(())
            })
            .is_some();
        if removed {
            self.count.fetch_sub(1, Ordering::Relaxed);
            self.chunks
                .remove_if(&chunk, |_, positions| positions.is_empty());
        }
    }

    pub fn remove_chunk(&self, chunk: &Vector2<i32>) {
        if let Some((_, positions)) = self.chunks.remove(chunk) {
            self.count.fetch_sub(positions.len(), Ordering::Relaxed);
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.count.load(Ordering::Relaxed) == 0
    }

    /// The listener positions in `chunk` between the section heights, section by section.
    fn positions_in(
        &self,
        chunk: Vector2<i32>,
        min_section: i32,
        max_section: i32,
    ) -> Vec<BlockPos> {
        let Some(positions) = self.chunks.get(&chunk) else {
            return Vec::new();
        };
        let mut positions: Vec<BlockPos> = positions
            .iter()
            .filter(|pos| (min_section..=max_section).contains(&(pos.0.y >> 4)))
            .copied()
            .collect();
        positions.sort_by_key(|pos| pos.0.y >> 4);
        positions
    }
}

/// Distance past a listener's radius at which an entity's body still counts as able to reach
/// it: some events are emitted a little away from the emitter, like a block it changes.
const LISTENER_REACH_MARGIN: f64 = 2.0;
/// Chunks around a ticking entity searched for block listeners it can reach.
const LISTENER_SEARCH_CHUNKS: i32 = 2;

impl World {
    /// Island stand-ins for the block listeners the entities in `chunks` can reach. Every
    /// entity whose body overlaps a listener's range joins its island, so the vibrations it
    /// selects and the blocks its range reads follow list order.
    pub(crate) fn block_listener_stand_ins(
        &self,
        chunks: impl Iterator<Item = Vector2<i32>>,
    ) -> Vec<(i32, entity_islands::TickReach)> {
        if self.block_listeners.is_empty() {
            return Vec::new();
        }
        let centers: rustc_hash::FxHashSet<_> = chunks.collect();
        // Far fewer chunks hold listeners than lie around the entities, so each listener chunk
        // looks for an entity chunk near it.
        let searched: Vec<_> = self
            .block_listeners
            .chunks
            .iter()
            .filter(|entry| {
                let chunk = entry.key();
                (-LISTENER_SEARCH_CHUNKS..=LISTENER_SEARCH_CHUNKS).any(|x| {
                    (-LISTENER_SEARCH_CHUNKS..=LISTENER_SEARCH_CHUNKS)
                        .any(|z| centers.contains(&Vector2::new(chunk.x + x, chunk.y + z)))
                })
            })
            .map(|entry| (*entry.key(), entry.value().clone()))
            .collect();
        let mut stand_ins = Vec::new();
        for (chunk, positions) in searched {
            let Some(block_entities) = self.block_entities.get(&chunk) else {
                continue;
            };
            for pos in positions {
                let Some(listener) = block_entities.get(&pos).and_then(|block_entity| {
                    block_entity
                        .game_event_listener()
                        .map(GameEventListener::listener_radius)
                }) else {
                    continue;
                };
                let center = pos.to_centered_f64();
                let range = f64::from(listener) + 1.0 + LISTENER_REACH_MARGIN;
                let hearing = BoundingBox::new(center, center).expand(range, range, range);
                // Below every entity id and every `!id` of a ticking entity.
                let id = i32::MIN + stand_ins.len() as i32;
                stand_ins.push((
                    id,
                    entity_islands::TickReach {
                        body: BoundingBox::new(center, center),
                        entities: hearing,
                        players: BoundingBox::new(center, center),
                        typed_entities: Vec::new(),
                        kind: Some(&pumpkin_data::entity::EntityType::MARKER),
                        block_reads: hearing,
                        block_writes: None,
                        global: false,
                        links: Vec::new(),
                        sight: Vec::new(),
                        spans_contacts: false,
                    },
                ));
            }
        }
        stand_ins
    }
}

fn notified_sections(center: BlockPos, radius: i32) -> (Vector3<i32>, Vector3<i32>) {
    (
        chunk_section_from_pos(&center.offset(Vector3::new(-radius, -radius, -radius))),
        chunk_section_from_pos(&center.offset(Vector3::new(radius, radius, radius))),
    )
}

/// The chunk sections whose listeners an event at `position` can notify.
#[must_use]
pub fn notified_area(position: Vector3<f64>, radius: i32) -> BoundingBox {
    let (section_min, section_max) = notified_sections(BlockPos::floored_v(position), radius);
    let block_min = |section: i32| f64::from(section_to_block(section));
    BoundingBox::new(
        Vector3::new(
            block_min(section_min.x),
            block_min(section_min.y),
            block_min(section_min.z),
        ),
        Vector3::new(
            block_min(section_max.x + 1),
            block_min(section_max.y + 1),
            block_min(section_max.z + 1),
        ),
    )
}

impl World {
    pub(super) fn post_game_event(
        &self,
        event: GameEvent,
        position: Vector3<f64>,
        context: &GameEventContext<'_>,
    ) {
        let radius = event.notification_radius();
        let center = BlockPos::floored_v(position);
        let (section_min, section_max) = notified_sections(center, radius);
        let area = notified_area(position, radius);
        for entity in self.entity_index.listeners(&self.entities, area) {
            let Some(mob) = entity.get_mob() else {
                continue;
            };
            if !self
                .level
                .is_chunk_loaded(&entity.get_entity().chunk_pos.load())
            {
                continue;
            }
            mob.for_each_game_event_listener(&mut |listener| {
                let listener_block = BlockPos::floored_v(listener.listener_position());
                let section = chunk_section_from_pos(&listener_block);
                let listener_radius = listener.listener_radius();
                if section.x < section_min.x
                    || section.y < section_min.y
                    || section.z < section_min.z
                    || section.x > section_max.x
                    || section.y > section_max.y
                    || section.z > section_max.z
                    || listener_block.squared_distance(&center) > listener_radius * listener_radius
                {
                    return;
                }
                entity_islands::check_reached(entity.get_entity().entity_id);
                listener.handle_game_event(self, event, context, position);
            });
        }
        self.post_game_event_to_blocks(event, position, context, center, section_min, section_max);
    }

    /// The block entity half of vanilla `GameEventDispatcher.post`.
    fn post_game_event_to_blocks(
        &self,
        event: GameEvent,
        position: Vector3<f64>,
        context: &GameEventContext<'_>,
        center: BlockPos,
        section_min: Vector3<i32>,
        section_max: Vector3<i32>,
    ) {
        if self.block_listeners.is_empty() {
            return;
        }
        let mut by_distance: Vec<(f64, Arc<dyn BlockEntity>)> = Vec::new();
        for chunk_x in section_min.x..=section_max.x {
            for chunk_z in section_min.z..=section_max.z {
                let chunk = Vector2::new(chunk_x, chunk_z);
                let positions =
                    self.block_listeners
                        .positions_in(chunk, section_min.y, section_max.y);
                if positions.is_empty() {
                    continue;
                }
                // Collected first: handling an event can add or remove block entities.
                let listeners: Vec<Arc<dyn BlockEntity>> = {
                    let Some(block_entities) = self.block_entities.get(&chunk) else {
                        continue;
                    };
                    positions
                        .iter()
                        .filter_map(|pos| block_entities.get(pos).cloned())
                        .collect()
                };
                for block_entity in listeners {
                    let Some(listener) = block_entity.game_event_listener() else {
                        continue;
                    };
                    let listener_position = listener.listener_position();
                    let radius = listener.listener_radius();
                    if BlockPos::floored_v(listener_position).squared_distance(&center)
                        > radius * radius
                    {
                        continue;
                    }
                    if listener.delivered_by_distance() {
                        by_distance.push((
                            position.squared_distance_to_vec(&listener_position),
                            block_entity,
                        ));
                    } else {
                        listener.handle_game_event(self, event, context, position);
                    }
                }
            }
        }
        by_distance.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (_, block_entity) in by_distance {
            if let Some(listener) = block_entity.game_event_listener() {
                listener.handle_game_event(self, event, context, position);
            }
        }
    }
}
