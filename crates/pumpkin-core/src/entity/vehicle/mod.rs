pub mod boat;
pub mod dismount;
pub mod minecart;
#[allow(clippy::module_inception)]
pub mod vehicle;

use std::sync::Arc;

use pumpkin_data::game_event::GameEvent;
use pumpkin_inventory::screen_handler::InventoryPlayer;

use crate::entity::mob::piglin_ai::PiglinAi;
use crate::entity::{Entity, player::Player};
use crate::world::game_event::GameEventContext;

/// Vanilla `VehicleEntity.getDimensionChangingDelay`, the portal cooldown of boats and minecarts.
pub(crate) const DIMENSION_CHANGING_DELAY: u32 = 10;

/// What vanilla `MinecartChest.interact` and `AbstractChestBoat` do once a player opens the
/// chest: a `container_open` game event, and piglins that see the player get angry.
pub(crate) fn chest_vehicle_opened(entity: &Entity, player: &Arc<Player>) {
    let world = entity.world.load();
    world.emit_game_event_with_context(
        GameEvent::ContainerOpen.name(),
        entity.pos.load(),
        &GameEventContext {
            source_entity: Some(player.as_ref()),
            affected_state: None,
        },
    );
    PiglinAi::anger_nearby_piglins(&world, player, true);
}

/// Vanilla `AbstractChestBoat.stopOpen` and `MinecartChest.stopOpen`: a `container_close` game
/// event at the vehicle from the player who closed it.
pub(crate) fn chest_vehicle_closed(entity_id: i32, player: &dyn InventoryPlayer) {
    let Some(player) = player.as_any().downcast_ref::<Player>() else {
        return;
    };
    let world = player.world();
    let Some(vehicle) = world.get_entity_by_id(entity_id) else {
        return;
    };
    world.emit_game_event_with_context(
        GameEvent::ContainerClose.name(),
        vehicle.get_entity().pos.load(),
        &GameEventContext {
            source_entity: Some(player),
            affected_state: None,
        },
    );
}

/// Vanilla `ContainerEntity.chestVehicleDestroyed`: a player breaking a chest vehicle angers
/// the piglins that see them.
pub(crate) fn chest_vehicle_destroyed(
    entity: &Entity,
    source: Option<&dyn crate::entity::EntityBase>,
) {
    if let Some(player) = source.and_then(crate::entity::EntityBase::get_player)
        && let Some(player) = entity.world.load().get_player_by_id(player.entity_id())
    {
        PiglinAi::anger_nearby_piglins(&entity.world.load(), &player, true);
    }
}
