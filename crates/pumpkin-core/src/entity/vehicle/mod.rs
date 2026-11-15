pub mod boat;
pub mod dismount;
pub mod minecart;
#[allow(clippy::module_inception)]
pub mod vehicle;

use std::sync::Arc;

use pumpkin_data::game_event::GameEvent;

use crate::entity::mob::piglin_ai::PiglinAi;
use crate::entity::{Entity, player::Player};
use crate::world::game_event::GameEventContext;

/// What vanilla `MinecartChest.interact` and `AbstractChestBoat` do once a player opens the
/// chest: a `container_open` game event, and piglins that see the player get angry.
pub(crate) fn chest_vehicle_opened(entity: &Entity, player: &Arc<Player>) {
    let world = entity.world.load();
    world.emit_game_event_with_context(
        GameEvent::ContainerOpen.name(),
        entity.pos.load(),
        &GameEventContext {
            source_entity: Some(player.as_ref()),
        },
    );
    PiglinAi::anger_nearby_piglins(&world, player, true);
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
