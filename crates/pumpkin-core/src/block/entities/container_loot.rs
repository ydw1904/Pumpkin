use std::sync::{Arc, Mutex, OnceLock, PoisonError, Weak};

use pumpkin_data::data_component_impl::ContainerLootImpl;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_inventory::Inventory;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

use crate::block::entities::BlockEntity;
use crate::entity::player::Player;
use crate::entity::player::advancement::trigger::AdvancementTrigger;
use crate::world::World;
use crate::world::loot::{LootContextParameters, fill_chest_inventory_handle};

const LOOT_TABLE_TAG: &str = "LootTable";
const LOOT_TABLE_SEED_TAG: &str = "LootTableSeed";

/// Vanilla `RandomizableContainer`: a loot table the container fills itself from the first
/// time anything reads or changes its contents.
#[derive(Default)]
pub struct ContainerLoot {
    table: Mutex<Option<(String, i64)>>,
    /// Vanilla `BlockEntity.level`, which the accessors unpack into.
    world: OnceLock<Weak<World>>,
}

impl ContainerLoot {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            table: Mutex::new(None),
            world: OnceLock::new(),
        }
    }

    /// Vanilla `tryLoadLootTable`. Returns whether the tag holds a loot table, in which case
    /// the container's items are not read.
    pub fn read_nbt(&self, nbt: &NbtCompound) -> bool {
        let table = nbt.get_string(LOOT_TABLE_TAG).map(|key| {
            (
                key.to_string(),
                nbt.get_long(LOOT_TABLE_SEED_TAG).unwrap_or(0),
            )
        });
        let has_table = table.is_some();
        *self.table.lock().unwrap_or_else(PoisonError::into_inner) = table;
        has_table
    }

    /// Vanilla `trySaveLootTable`. Returns whether a loot table was written, in which case the
    /// container's items are not.
    pub fn write_nbt(&self, nbt: &mut NbtCompound) -> bool {
        let Some((key, seed)) = self.get() else {
            return false;
        };
        nbt.put_string(LOOT_TABLE_TAG, key);
        if seed != 0 {
            nbt.put_long(LOOT_TABLE_SEED_TAG, seed);
        }
        true
    }

    #[must_use]
    pub fn get(&self) -> Option<(String, i64)> {
        self.table
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn set(&self, table: Option<(String, i64)>) {
        *self.table.lock().unwrap_or_else(PoisonError::into_inner) = table;
    }

    #[must_use]
    pub fn is_pending(&self) -> bool {
        self.table
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    }

    /// Vanilla `BlockEntity.setLevel`.
    pub fn set_world(&self, world: &Arc<World>) {
        self.world.get_or_init(|| Arc::downgrade(world));
    }

    /// Vanilla `RandomizableContainerBlockEntity.applyImplicitComponents`.
    pub fn apply_item_components(&self, stack: &ItemStack) {
        if let Some(loot) = stack.get_data_component::<ContainerLootImpl>() {
            self.set(Some((loot.loot_table.clone(), loot.seed)));
        }
    }

    /// Vanilla `RandomizableContainer.unpackLootTable(null)`, which every container accessor
    /// runs first. Does nothing before the block entity is in a world, like vanilla.
    pub fn unpack(&self, container: &dyn Inventory, position: &BlockPos) {
        if !self.is_pending() {
            return;
        }
        if let Some(world) = self.world.get().and_then(Weak::upgrade) {
            self.unpack_in(&world, container, position.to_centered_f64(), None);
        }
    }

    /// Vanilla `RandomizableContainer.unpackLootTable` and
    /// `ContainerEntity.unpackChestVehicleLootTable`.
    pub fn unpack_in(
        &self,
        world: &World,
        container: &dyn Inventory,
        origin: Vector3<f64>,
        player: Option<&Player>,
    ) {
        // Cleared before filling, so the accessors the fill goes through don't unpack again.
        let Some((key, seed)) = self
            .table
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        else {
            return;
        };
        if let Some(player) = player {
            player.trigger_advancement(AdvancementTrigger::PlayerGeneratesContainerLoot {
                loot_table_id: key.clone(),
            });
        }
        // An unknown table is vanilla's empty one: the container stays empty.
        let Some(table) = world.get_loot_table(&key) else {
            return;
        };
        let params = LootContextParameters {
            world: Some(world),
            position: Some(origin),
            this_entity: player.map(|player| player as &dyn crate::entity::EntityBase),
            luck: player.map_or(0.0, |player| {
                player
                    .living_entity
                    .get_attribute_value(&pumpkin_data::attributes::Attributes::LUCK)
                    as f32
            }),
            ..Default::default()
        };
        fill_chest_inventory_handle(container, &table, seed, &params);
        container.mark_dirty();
    }
}

/// Whether the block entity is a container whose loot table hasn't been unpacked yet.
#[must_use]
pub fn has_pending_loot(entity: &dyn BlockEntity) -> bool {
    entity
        .container_loot()
        .is_some_and(ContainerLoot::is_pending)
}

/// Vanilla `RandomizableContainer.unpackLootTable(player)` for a container block entity.
///
/// Spectators never unpack; [`open_container`] also keeps them out of pending containers.
pub fn unpack_for_player(entity: &Arc<dyn BlockEntity>, world: &World, player: &Player) {
    if player.is_spectator() {
        return;
    }
    let (Some(loot), Some(inventory)) = (entity.container_loot(), entity.clone().get_inventory())
    else {
        return;
    };
    loot.unpack_in(
        world,
        inventory.as_ref(),
        entity.get_position().to_centered_f64(),
        Some(player),
    );
}

/// Vanilla `RandomizableContainerBlockEntity.createMenu`: spectators can't open a container
/// whose loot is still pending, anyone else unpacks it. Returns whether the menu may open.
pub fn open_container(entity: &Arc<dyn BlockEntity>, world: &World, player: &Player) -> bool {
    if player.is_spectator() && has_pending_loot(entity.as_ref()) {
        return false;
    }
    unpack_for_player(entity, world, player);
    true
}
