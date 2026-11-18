use std::sync::Arc;
use std::sync::atomic::Ordering;

use pumpkin_data::entity::EntityType;
use pumpkin_macros::pumpkin_block;
use pumpkin_util::math::{position::BlockPos, vector3::Vector3};

use crate::block::entities::end_gateway::{EndGatewayBlockEntity, ExitChunkTickets};
use crate::block::{BlockBehaviour, OnEntityCollisionArgs, OnSyncedBlockEventArgs, PlacedArgs};
use crate::entity::{EntityBase, teleport_with_passengers};
use crate::world::{TimedTicketType, World};

#[pumpkin_block("minecraft:end_gateway")]
pub struct EndGatewayBlock;

/// The teleport of vanilla `EndGatewayBlock.getPortalDestination`: ender pearls stop and face
/// south, everything else keeps its motion and rotation, and a vehicle takes its passengers.
fn teleport_through(
    gateway: &EndGatewayBlockEntity,
    traveller: &dyn EntityBase,
    world: &Arc<World>,
) {
    let Some(destination) = gateway.get_portal_position(world) else {
        return;
    };
    let entity = traveller.get_entity();
    let (yaw, pitch) = if entity.entity_type == &EntityType::ENDER_PEARL {
        entity.set_velocity(Vector3::new(0.0, 0.0, 0.0));
        (0.0, 0.0)
    } else {
        (entity.yaw.load(), entity.pitch.load())
    };
    teleport_with_passengers(traveller, destination, yaw, pitch, world);
    // Vanilla `TeleportTransition.PLACE_PORTAL_TICKET`.
    world.add_timed_ticket(
        TimedTicketType::Portal,
        BlockPos::floored_v(destination).chunk_position(),
    );
}

impl BlockBehaviour for EndGatewayBlock {
    /// Vanilla `EndGatewayBlock.entityInside` followed by the portal teleport: passengers can't
    /// use it, an entity on portal cooldown only renews its cooldown, and a vehicle takes its
    /// passengers along.
    fn on_entity_collision(&self, args: OnEntityCollisionArgs<'_>) {
        let entity = args.entity.get_entity();
        if !args.entity.can_use_portal() || entity.has_vehicle() || !entity.is_alive() {
            return;
        }

        let Some(block_entity) = args.world.get_block_entity(args.position) else {
            return;
        };
        let Some(end_gateway) = block_entity
            .as_any()
            .downcast_ref::<EndGatewayBlockEntity>()
        else {
            return;
        };

        if end_gateway.is_cooling_down() {
            return;
        }

        let on_cooldown = entity.portal_cooldown.load(Ordering::Relaxed) > 0;
        end_gateway.trigger_cooldown(args.world, *args.position);
        entity.portal_cooldown.store(
            args.entity.get_dimension_changing_delay(),
            Ordering::Relaxed,
        );
        if on_cooldown {
            return;
        }

        if !end_gateway.needs_exit(args.world) && end_gateway.exit_position_loaded(args.world) {
            teleport_through(end_gateway, args.entity, args.world);
            return;
        }
        // The exit search reads chunks that may not be loaded yet, so it runs off the tick and
        // the entity travels once the exit exists and its surroundings are loaded.
        let (Some(server), Some(traveller)) = (
            args.world.server.upgrade(),
            args.world.get_entity_by_id(entity.entity_id),
        ) else {
            return;
        };
        let world = args.world.clone();
        let position = *args.position;
        server.spawn_task(async move {
            let Some(block_entity) = world.get_block_entity(&position) else {
                return;
            };
            let Some(end_gateway) = block_entity
                .as_any()
                .downcast_ref::<EndGatewayBlockEntity>()
            else {
                return;
            };
            let mut tickets = ExitChunkTickets::new(world.level.clone());
            if end_gateway.needs_exit(&world) {
                end_gateway
                    .create_exit(&world, position, &mut tickets)
                    .await;
            }
            end_gateway.load_exit_position_chunks(&mut tickets).await;
            if traveller.get_entity().is_alive() {
                teleport_through(end_gateway, traveller.as_ref(), &world);
            }
        });
    }

    fn on_synced_block_event(&self, args: OnSyncedBlockEventArgs<'_>) -> bool {
        if let Some(block_entity) = args.world.get_block_entity(args.position)
            && let Some(end_gateway) = block_entity
                .as_any()
                .downcast_ref::<EndGatewayBlockEntity>()
        {
            end_gateway.trigger_event(args.r#type, args.data)
        } else {
            args.r#type == EndGatewayBlockEntity::EVENT_COOLDOWN
        }
    }

    fn placed(&self, args: PlacedArgs<'_>) {
        let block_entity = EndGatewayBlockEntity::new(*args.position);
        args.world.add_block_entity(Arc::new(block_entity));
    }
}
