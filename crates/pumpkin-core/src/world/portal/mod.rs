use std::sync::Arc;
use std::sync::atomic::Ordering;

use pumpkin_data::Block;
use pumpkin_data::block_properties::HorizontalAxis;
use pumpkin_data::dimension::Dimension;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector2::Vector2;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::chunk::ChunkHeightmapType;
use pumpkin_world::world::BlockFlags;

use super::{TimedTicketType, World};

pub mod end;
pub mod nether;
pub mod poi;

pub use nether::{NetherPortal, PortalSearchResult};
pub use poi::PortalPoiStorage;

#[derive(Clone)]
pub struct SourcePortalInfo {
    pub lower_corner: BlockPos,
    pub axis: HorizontalAxis,
    pub width: u32,
    pub height: u32,
}

impl From<&PortalSearchResult> for SourcePortalInfo {
    fn from(result: &PortalSearchResult) -> Self {
        Self {
            lower_corner: result.lower_corner,
            axis: result.axis,
            width: result.width,
            height: result.height,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PortalType {
    Nether,
    End,
}

impl PortalType {
    pub fn get_portal_transition_time(
        &self,
        current_world: &World,
        entity: &dyn crate::entity::EntityBase,
    ) -> u32 {
        match self {
            Self::End => 0,
            // Vanilla `NetherPortalBlock.getPortalTransitionTime`.
            Self::Nether => entity.get_player().map_or(0, |player| {
                let invulnerable = player
                    .abilities
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .invulnerable;
                let rules = &current_world.level_info.load().game_rules;
                let delay = if invulnerable {
                    rules.players_nether_portal_creative_delay
                } else {
                    rules.players_nether_portal_default_delay
                };
                u32::try_from(delay.max(0)).unwrap_or(u32::MAX)
            }),
        }
    }

    #[expect(clippy::too_many_lines)]
    pub async fn get_portal_destination(
        &self,
        current_level: &World,
        dest_world: Arc<World>,
        caller: &dyn crate::entity::EntityBase,
        source_portal: Option<&SourcePortalInfo>,
    ) -> Option<TeleportTransition> {
        match self {
            Self::End => {
                let is_end_portal = dest_world.dimension == Dimension::THE_END
                    || current_level.dimension == Dimension::THE_END;

                if is_end_portal {
                    if dest_world.dimension == Dimension::THE_END {
                        // Entering the End: spawn on the obsidian platform at (100, 49, 0) for players, or (100, 50, 0) for other entities
                        let is_player = caller
                            .get_living_entity()
                            .is_some_and(crate::entity::living::LivingEntity::is_player);
                        let y = if is_player { 49.0 } else { 50.0 };

                        let platform_pos = BlockPos::new(100, 49, 0);

                        // Load the chunks the platform spans.
                        for chunk_x in (platform_pos.0.x - 2) >> 4..=(platform_pos.0.x + 2) >> 4 {
                            for chunk_z in (platform_pos.0.z - 2) >> 4..=(platform_pos.0.z + 2) >> 4
                            {
                                dest_world
                                    .level
                                    .get_or_fetch_chunk(Vector2::new(chunk_x, chunk_z), |_| ())
                                    .await;
                            }
                        }

                        // Vanilla `EndPlatformFeature.createEndPlatform(level, origin, true)`:
                        // anything else in the way is destroyed and drops. Entities arriving
                        // together take turns, so each block drops once.
                        let platform_lock = dest_world.portal_forcer.lock().await;
                        for dz in -2..=2 {
                            for dx in -2..=2 {
                                for dy in -1..3 {
                                    let block = if dy == -1 {
                                        &Block::OBSIDIAN
                                    } else {
                                        &Block::AIR
                                    };
                                    let target_pos = BlockPos::new(
                                        platform_pos.0.x + dx,
                                        platform_pos.0.y + dy,
                                        platform_pos.0.z + dz,
                                    );
                                    if dest_world.get_block(&target_pos) != block {
                                        dest_world.destroy_block(
                                            &target_pos,
                                            None,
                                            BlockFlags::NOTIFY_ALL,
                                        );
                                        dest_world.set_block_state(
                                            &target_pos,
                                            block.default_state.id,
                                            BlockFlags::NOTIFY_ALL,
                                        );
                                    }
                                }
                            }
                        }

                        drop(platform_lock);

                        // Vanilla faces west and keeps the pitch (`Relative.X_ROT`).
                        let position = Vector3::new(100.5f64, y, 0.5f64);
                        Some(TeleportTransition {
                            new_world: dest_world,
                            position,
                            yaw: Some(90.0f32),
                            pitch: Some(caller.get_entity().pitch.load()),
                            portal_ticket: Some(BlockPos::floored_v(position)),
                        })
                    } else {
                        // Leaving the End through the exit portal. Vanilla `EndPortalBlock`
                        // shows the credits instead the first time (`showEndCredits`); a Java
                        // player then leaves the level until the client asks to respawn.
                        // Bedrock has no respawn request after the credits, so it travels
                        // straight away.
                        if let Some(player) =
                            current_level.get_player_by_id(caller.get_entity().entity_id)
                        {
                            if player.won_game.load(Ordering::Relaxed) {
                                return None;
                            }
                            if !player.seen_credits.swap(true, Ordering::Relaxed) {
                                match player.client.as_ref() {
                                    crate::net::ClientPlatform::Java(client) => {
                                        if let Ok(data) = client.serialize_packet(&pumpkin_protocol::java::client::play::CGameEvent::new(
                                            pumpkin_protocol::java::client::play::GameEvent::WinGame,
                                            0.0,
                                        )) {
                                            client.try_enqueue_packet(data);
                                        }
                                        player.won_game.store(true, Ordering::Relaxed);
                                        // Vanilla `removePlayerImmediately`: nobody sees the
                                        // player while the credits run.
                                        current_level.entity_tracker.remove_entity(
                                            player.as_ref() as &dyn crate::entity::EntityBase,
                                            current_level,
                                        );
                                        return None;
                                    }
                                    crate::net::ClientPlatform::Bedrock(client) => {
                                        if let Ok(data) = client.serialize_packet(
                                            &pumpkin_protocol::bedrock::client::CShowCredits {
                                                player_runtime_id: (caller.get_entity().entity_id
                                                    as u64)
                                                    .into(),
                                                credits_state: 0.into(),
                                            },
                                        ) {
                                            client.try_enqueue_packet(data);
                                        }
                                    }
                                }
                            }

                            // Vanilla `findRespawnPositionAndUseSpawnBlock(false, ...)`: the bed
                            // or charged anchor, without using up a charge.
                            if let Some(respawn) = player.calculate_respawn_point(false).await
                                && let Some(server) = current_level.server.upgrade()
                            {
                                return Some(TeleportTransition {
                                    new_world: server.get_world_from_dimension(&respawn.dimension),
                                    position: respawn.position,
                                    yaw: Some(respawn.yaw),
                                    pitch: Some(respawn.pitch),
                                    portal_ticket: None,
                                });
                            }
                            let missing_respawn_block = player
                                .respawn_point
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .is_some();
                            if missing_respawn_block {
                                player
                                    .send_client_packet(
                                        &pumpkin_protocol::java::client::play::CGameEvent::new(
                                            pumpkin_protocol::java::client::play::GameEvent::NoRespawnBlockAvailable,
                                            0.0,
                                        ),
                                    )
                                    .await;
                            }

                            // Vanilla `TeleportTransition.createDefault`: a safe spot around the
                            // world spawn, facing the spawn angle.
                            let (spawn_x, spawn_y, spawn_z, yaw, pitch) = {
                                let info = dest_world.level_info.load();
                                (
                                    info.spawn_x,
                                    info.spawn_y,
                                    info.spawn_z,
                                    info.spawn_yaw,
                                    info.spawn_pitch,
                                )
                            };
                            let position = dest_world
                                .get_safe_player_spawn_position(spawn_x, spawn_z, spawn_y)
                                .await;
                            return Some(TeleportTransition {
                                new_world: dest_world,
                                position,
                                yaw: Some(yaw),
                                pitch: Some(pitch),
                                portal_ticket: None,
                            });
                        }

                        // Vanilla `Entity.adjustSpawnLocation`: on top of the world spawn column.
                        let (spawn_x, spawn_z, spawn_yaw, spawn_pitch) = {
                            let info = dest_world.level_info.load();
                            (info.spawn_x, info.spawn_z, info.spawn_yaw, info.spawn_pitch)
                        };
                        dest_world
                            .level
                            .get_or_fetch_chunk(Vector2::new(spawn_x >> 4, spawn_z >> 4), |_| ())
                            .await;
                        let spawn_y = dest_world.get_heightmap_height(
                            ChunkHeightmapType::MotionBlockingNoLeaves,
                            spawn_x,
                            spawn_z,
                        ) + 1;
                        let position = Vector3::new(
                            f64::from(spawn_x) + 0.5,
                            f64::from(spawn_y),
                            f64::from(spawn_z) + 0.5,
                        );
                        Some(TeleportTransition {
                            new_world: dest_world,
                            position,
                            portal_ticket: Some(BlockPos::floored_v(position)),
                            // Vanilla `Relative.ROTATION`: the spawn angle is added on.
                            yaw: Some(caller.get_entity().yaw.load() + spawn_yaw),
                            pitch: Some(
                                (caller.get_entity().pitch.load() + spawn_pitch).clamp(-90.0, 90.0),
                            ),
                        })
                    }
                } else {
                    None
                }
            }
            Self::Nether => {
                let pos = caller.get_entity().pos.load();
                let current_yaw = caller.get_entity().yaw.load();
                let dimensions = caller.get_entity().entity_dimension.load();
                let scale_factor_new = dest_world.dimension.coordinate_scale;
                let scale_factor_current = current_level.dimension.coordinate_scale;

                let teleportation_scale = scale_factor_current / scale_factor_new;
                let (clamped_x, clamped_z) = {
                    let worldborder = dest_world
                        .worldborder
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    worldborder.clamp_block(
                        (pos.x * teleportation_scale).floor() as i32,
                        (pos.z * teleportation_scale).floor() as i32,
                    )
                };

                let approximate_exit_pos =
                    BlockPos::new(clamped_x, pos.y.floor() as i32, clamped_z);
                let source_portal_axis = source_portal.map_or(HorizontalAxis::X, |p| p.axis);

                let forcer = dest_world.portal_forcer.lock().await;
                let mut exit_portal =
                    NetherPortal::search_for_portal(&dest_world, approximate_exit_pos).await;
                // Vanilla tickets a portal it found, or else where the entity lands.
                let found_portal = exit_portal.as_ref().map(|portal| portal.lower_corner);

                if exit_portal.is_none() {
                    let center_chunk =
                        Vector2::new(approximate_exit_pos.0.x >> 4, approximate_exit_pos.0.z >> 4);
                    for dx in -1..=1 {
                        for dz in -1..=1 {
                            let chunk_pos = Vector2::new(center_chunk.x + dx, center_chunk.y + dz);
                            dest_world.level.get_or_fetch_chunk(chunk_pos, |_| ()).await;
                        }
                    }

                    if let Some(build_pos) = NetherPortal::create_portal(
                        &dest_world,
                        approximate_exit_pos,
                        source_portal_axis,
                    ) {
                        exit_portal = Some(PortalSearchResult {
                            lower_corner: build_pos,
                            axis: source_portal_axis,
                            width: 2,
                            height: 3,
                        });
                    }
                }

                drop(forcer);

                let (final_pos, yaw) = exit_portal.map_or_else(
                    || (approximate_exit_pos.0.to_f64(), None),
                    |exit_portal| {
                        let relative_offset = source_portal.map_or_else(
                            || Vector3::new(0.5, 0.0, 0.0),
                            |source| {
                                let source_result = PortalSearchResult {
                                    lower_corner: source.lower_corner,
                                    axis: source.axis,
                                    width: source.width,
                                    height: source.height,
                                };
                                source_result.entity_pos_in_portal(pos, &dimensions)
                            },
                        );
                        let target_pos =
                            exit_portal.calculate_exit_position(relative_offset, &dimensions);
                        let collision_free_pos =
                            exit_portal.find_open_position(&dest_world, target_pos, &dimensions);
                        let yaw = exit_portal
                            .calculate_teleport_yaw(current_yaw, source_portal.map(|p| p.axis));
                        (collision_free_pos, Some(yaw))
                    },
                );

                Some(TeleportTransition {
                    new_world: dest_world,
                    position: final_pos,
                    yaw,
                    pitch: None,
                    portal_ticket: Some(
                        found_portal.unwrap_or_else(|| BlockPos::floored_v(final_pos)),
                    ),
                })
            }
        }
    }
}

pub struct TeleportTransition {
    pub new_world: Arc<World>,
    pub position: Vector3<f64>,
    pub yaw: Option<f32>,
    pub pitch: Option<f32>,
    /// Vanilla `TeleportTransition.PLACE_PORTAL_TICKET`: where the arrival is held with a
    /// portal ticket.
    pub portal_ticket: Option<BlockPos>,
}

impl TeleportTransition {
    /// Runs the post-teleport part of the transition, once the entity has arrived.
    pub fn place_portal_ticket(&self) {
        if let Some(pos) = self.portal_ticket {
            self.new_world
                .add_timed_ticket(TimedTicketType::Portal, pos.chunk_position());
        }
    }
}

pub struct PortalProcessor {
    pub portal_type: PortalType,
    pub entry_position: BlockPos,
    pub portal_time: u32,
    pub inside_portal_this_tick: bool,
    pub destination_world: Arc<World>,
    pub source_portal: Option<SourcePortalInfo>,
}

impl PortalProcessor {
    pub const fn new(
        portal_type: PortalType,
        entry_position: BlockPos,
        destination_world: Arc<World>,
    ) -> Self {
        Self {
            portal_type,
            entry_position,
            portal_time: 0,
            inside_portal_this_tick: true,
            destination_world,
            source_portal: None,
        }
    }

    pub const fn set_source_portal(&mut self, info: SourcePortalInfo) {
        self.source_portal = Some(info);
    }

    pub fn process_portal_teleportation(
        &mut self,
        current_world: &World,
        entity: &dyn crate::entity::EntityBase,
        allowed_to_teleport: bool,
    ) -> bool {
        if self.inside_portal_this_tick {
            self.inside_portal_this_tick = false;
            if allowed_to_teleport {
                // Vanilla `portalTime++ >= getPortalTransitionTime(...)`.
                let portal_time = self.portal_time;
                self.portal_time += 1;
                portal_time
                    >= self
                        .portal_type
                        .get_portal_transition_time(current_world, entity)
            } else {
                false
            }
        } else {
            self.decay_tick();
            false
        }
    }

    pub const fn decay_tick(&mut self) {
        self.portal_time = self.portal_time.saturating_sub(4);
    }

    #[must_use]
    pub const fn has_expired(&self) -> bool {
        self.portal_time == 0
    }
}
