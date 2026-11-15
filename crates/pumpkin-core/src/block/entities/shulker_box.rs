use pumpkin_data::entity::EntityType;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::Taggable;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::position::BlockPos;
use std::any::Any;
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::{array::from_fn, sync::Arc};

use crate::block::entities::BlockEntity;
use crate::block::entities::base_container::BaseContainer;
use crate::block::entities::container_loot::ContainerLoot;
use crate::block::viewer::{
    ViewerCountListener, ViewerCountTracker, ViewerCountTrackerExt, emit_container_event,
};
use crate::entity::item::ItemEntity;
use crate::entity::{Entity, EntityBase};
use crate::world::World;
use pumpkin_inventory::{Clearable, Inventory};

pub struct ShulkerBoxBlockEntity {
    pub position: BlockPos,
    pub items: RwLock<[ItemStack; Self::INVENTORY_SIZE]>,
    pub dirty: AtomicBool,
    pub comparator_dirty: AtomicBool,
    pub loot: ContainerLoot,
    pub base: BaseContainer,

    // Viewer
    pub viewers: ViewerCountTracker,
}

impl BlockEntity for ShulkerBoxBlockEntity {
    fn resource_location(&self) -> &'static str {
        Self::ID
    }

    fn get_position(&self) -> BlockPos {
        self.position
    }

    fn from_nbt(nbt: &pumpkin_nbt::compound::NbtCompound, position: BlockPos) -> Self
    where
        Self: Sized,
    {
        let mut shulker_box = Self {
            position,
            items: RwLock::new(from_fn(|_| ItemStack::EMPTY.clone())),
            dirty: AtomicBool::new(false),
            comparator_dirty: AtomicBool::new(false),
            loot: ContainerLoot::default(),
            base: BaseContainer::new(),
            viewers: ViewerCountTracker::new(),
        };

        shulker_box.base.read_nbt(nbt);

        if !shulker_box.loot.read_nbt(nbt) {
            pumpkin_inventory::sync_read_items_from_nbt(
                nbt,
                shulker_box
                    .items
                    .get_mut()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            );
        }

        shulker_box
    }

    fn write_nbt(&self, nbt: &mut NbtCompound) {
        self.base.write_nbt(nbt);
        if !self.loot.write_nbt(nbt) {
            self.write_inventory_nbt(nbt, true);
        }
    }

    fn tick(&self, world: &Arc<World>) {
        self.viewers
            .update_viewer_count::<Self>(self, world, &self.position);
    }

    fn on_block_replaced(self: Arc<Self>, _world: &Arc<World>, _position: &BlockPos) {
        // Shulker boxes retain items when broken
    }

    fn container_loot(&self) -> Option<&ContainerLoot> {
        Some(&self.loot)
    }

    fn get_inventory(self: Arc<Self>) -> Option<Arc<dyn Inventory>> {
        Some(self)
    }

    fn custom_name(&self) -> Option<pumpkin_util::text::TextComponent> {
        self.base.custom_name()
    }

    fn base_container(&self) -> Option<&crate::block::entities::base_container::BaseContainer> {
        Some(&self.base)
    }

    fn collect_item_components(&self, stack: &mut ItemStack) {
        self.base.collect_item_components(
            &*self
                .items
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            stack,
        );
        self.loot.collect_item_components(stack);
    }

    fn apply_item_components(&self, stack: &ItemStack) {
        self.base.apply_item_components(
            stack,
            &mut *self
                .items
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        self.loot.apply_item_components(stack);
        self.mark_dirty();
    }

    fn is_comparator_dirty(&self) -> bool {
        self.comparator_dirty.load(Ordering::Relaxed)
    }

    fn clear_comparator_dirty(&self) {
        self.comparator_dirty.store(false, Ordering::Relaxed);
    }

    fn is_dirty(&self) -> bool {
        self.dirty.load(Ordering::Relaxed)
    }

    fn clear_dirty(&self) {
        self.dirty.store(false, Ordering::Relaxed);
    }

    fn bedrock_block_actor_data(
        &self,
        state_id: pumpkin_data::BlockStateId,
    ) -> Option<NbtCompound> {
        let facing =
            match pumpkin_data::block_properties::ShulkerBoxProperties::from_state_id(state_id)
                .facing
            {
                pumpkin_data::block_properties::Facing::Down => 0,
                pumpkin_data::block_properties::Facing::Up => 1,
                pumpkin_data::block_properties::Facing::North => 2,
                pumpkin_data::block_properties::Facing::South => 3,
                pumpkin_data::block_properties::Facing::West => 4,
                pumpkin_data::block_properties::Facing::East => 5,
            };
        Some(build_shulker_box_actor(
            self.position,
            facing,
            self.base.bedrock_custom_name().as_deref(),
        ))
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl ViewerCountListener for ShulkerBoxBlockEntity {
    fn on_container_open(
        &self,
        world: &Arc<World>,
        position: &BlockPos,
        user: Option<&dyn EntityBase>,
    ) {
        Self::play_sound(world, position, 1);
        emit_container_event(world, GameEvent::ContainerOpen, position, user);
    }

    fn on_container_close(
        &self,
        world: &Arc<World>,
        position: &BlockPos,
        user: Option<&dyn EntityBase>,
    ) {
        Self::play_sound(world, position, 0);
        emit_container_event(world, GameEvent::ContainerClose, position, user);
    }

    fn on_viewer_count_update(&self, world: &Arc<World>, position: &BlockPos, _old: u16, new: u16) {
        world.add_synced_block_event(*position, Self::OPEN_ANIMATION_EVENT_TYPE, new as u8);
    }
}

impl ShulkerBoxBlockEntity {
    pub const INVENTORY_SIZE: usize = 27;
    pub const OPEN_ANIMATION_EVENT_TYPE: u8 = 1;
    pub const ID: &'static str = "minecraft:shulker_box";

    #[must_use]
    pub fn new(position: BlockPos) -> Self {
        Self {
            position,
            items: RwLock::new(from_fn(|_| ItemStack::EMPTY.clone())),
            dirty: AtomicBool::new(false),
            comparator_dirty: AtomicBool::new(false),
            loot: ContainerLoot::default(),
            base: BaseContainer::new(),
            viewers: ViewerCountTracker::new(),
        }
    }

    pub fn update_viewers(&self, world: &Arc<World>) {
        let viewer_count = self.viewers.current.load(Ordering::Relaxed);
        Self::play_sound(world, &self.position, i32::from(viewer_count));
    }

    fn play_sound(world: &World, position: &BlockPos, viewer_count: i32) {
        let sound = if viewer_count > 0 {
            Sound::BlockShulkerBoxOpen
        } else {
            Sound::BlockShulkerBoxClose
        };

        world.play_sound(sound, SoundCategory::Blocks, &position.to_f64());
    }
}

impl Inventory for ShulkerBoxBlockEntity {
    fn size(&self) -> usize {
        Self::INVENTORY_SIZE
    }

    fn is_empty(&self) -> bool {
        self.loot.unpack(self, &self.position);
        let items = self
            .items
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        items.iter().all(ItemStack::is_empty)
    }

    fn get_stack(&self, slot: usize) -> ItemStack {
        self.loot.unpack(self, &self.position);
        let items = self
            .items
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        items[slot].clone()
    }

    fn remove_stack(&self, slot: usize) -> ItemStack {
        self.loot.unpack(self, &self.position);
        let mut items = self
            .items
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let removed = std::mem::replace(&mut items[slot], ItemStack::EMPTY.clone());
        self.mark_dirty();
        removed
    }

    fn remove_stack_specific(&self, slot: usize, amount: u8) -> ItemStack {
        self.loot.unpack(self, &self.position);
        let mut items = self
            .items
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let res = if !items[slot].is_empty() && amount > 0 {
            items[slot].split(amount)
        } else {
            ItemStack::EMPTY.clone()
        };
        self.mark_dirty();
        res
    }

    fn set_stack(&self, slot: usize, stack: ItemStack) {
        self.loot.unpack(self, &self.position);
        let mut items = self
            .items
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        items[slot] = stack;
        self.mark_dirty();
    }

    fn is_valid_slot_for(&self, _slot: usize, stack: &ItemStack) -> bool {
        !stack
            .item
            .has_tag(&pumpkin_data::tag::Item::MINECRAFT_SHULKER_BOXES)
    }

    fn on_open(&self) {
        self.viewers.open_container();
    }

    fn on_close(&self) {
        self.viewers.close_container();
    }

    fn on_open_by_user(&self, user: i32) {
        self.viewers.open_container_by(user);
    }

    fn on_close_by_user(&self, user: i32) {
        self.viewers.close_container_by(user);
    }

    fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::Relaxed);
        self.comparator_dirty.store(true, Ordering::Relaxed);
    }

    /// Shulker boxes don't take other shulker boxes from hoppers or droppers either.
    fn can_place_item_through_face(
        &self,
        _slot: usize,
        stack: &ItemStack,
        _side: Option<pumpkin_data::BlockDirection>,
    ) -> bool {
        stack.can_fit_inside_container_items()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl Clearable for ShulkerBoxBlockEntity {
    fn clear(&self) {
        let mut items = self
            .items
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        items.fill_with(|| ItemStack::EMPTY.clone());
        self.mark_dirty();
    }
}

/// Builds the Bedrock block-actor NBT for shulker boxes.
#[must_use]
pub fn build_shulker_box_actor(
    position: BlockPos,
    facing: u8,
    custom_name: Option<&str>,
) -> NbtCompound {
    super::bedrock::BedrockBlockActorBuilder::new("ShulkerBox", position)
        .movable(false)
        .custom_name(custom_name)
        .put_byte("facing", facing as i8)
        .finish()
}

/// Vanilla `ShulkerBoxBlock.playerWillDestroy`: a creative player breaking a box that isn't
/// empty gets it as an item with all of its components, anyone else unpacks its loot table.
pub fn player_will_destroy(
    world: &Arc<World>,
    position: &BlockPos,
    player: &crate::entity::player::Player,
) {
    let Some(entity) = world.get_block_entity(position) else {
        return;
    };
    let Some(shulker_box) = entity.as_any().downcast_ref::<ShulkerBoxBlockEntity>() else {
        return;
    };
    // `isEmpty` unpacks a pending loot table without a player first.
    if player.gamemode.load() != pumpkin_util::GameMode::Creative || shulker_box.is_empty() {
        crate::block::entities::container_loot::unpack_for_player(&entity, world, player);
        return;
    }
    let Some(item) = Item::from_id(world.get_block(position).item_id) else {
        return;
    };
    let mut stack = ItemStack::new(1, item);
    world.collect_block_entity_components(entity.as_ref(), &mut stack);
    let item_entity = Entity::new(world.clone(), position.to_centered_f64(), &EntityType::ITEM);
    world.spawn_entity(Arc::new(ItemEntity::new(item_entity, stack)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shulker_box_actor_builder() {
        let pos = BlockPos::new(4, 64, 4);
        let nbt = build_shulker_box_actor(pos, 1, Some("Box"));
        assert_eq!(nbt.get_string("id"), Some("ShulkerBox"));
        assert_eq!(nbt.get_byte("facing"), Some(1));
        assert_eq!(nbt.get_bool("isMovable"), Some(false));
        assert_eq!(nbt.get_string("CustomName"), Some("Box"));
    }
}
