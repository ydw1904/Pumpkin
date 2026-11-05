use std::sync::Arc;

use pumpkin_data::entity::EntityType;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::SoundCategory;
use pumpkin_util::math::vector3::Vector3;

use crate::entity::Entity;
use crate::entity::item::ItemEntity;
use crate::entity::mob::Mob;
use crate::world::loot::{LootContextParameters, generate_loot_from_handle};

/// Vanilla `Shearable`: a mob that players and dispensers can shear.
pub trait Shearable {
    fn shear(&self, sound_category: SoundCategory, tool: &ItemStack);

    fn ready_for_shearing(&self) -> bool;
}

/// Vanilla `Entity.dropFromShearingLootTable` with `spawnAtLocation(level, drop, y_offset)`.
pub fn drop_from_shearing_loot_table(
    shearable: &dyn Mob,
    loot_table: &str,
    tool: &ItemStack,
    y_offset: f64,
) {
    let entity = shearable.get_entity();
    let world = entity.world.load();
    let Some(table) = world.get_loot_table(loot_table) else {
        return;
    };
    let pos = entity.pos.load();
    let params = LootContextParameters {
        world: Some(&world),
        position: Some(pos),
        this_entity: Some(shearable),
        tool: Some(tool.clone()),
        ..Default::default()
    };
    let seed = shearable.get_random().random();
    for stack in generate_loot_from_handle(&table, seed, &params) {
        if stack.is_empty() {
            continue;
        }
        let drop_pos = Vector3::new(pos.x, pos.y + y_offset, pos.z);
        world.spawn_entity(Arc::new(ItemEntity::new(
            Entity::new(world.clone(), drop_pos, &EntityType::ITEM),
            stack,
        )));
    }
}
