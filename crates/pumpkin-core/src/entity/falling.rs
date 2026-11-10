use crossbeam::atomic::AtomicCell;
use pumpkin_data::Block;
use pumpkin_data::BlockState;
use pumpkin_data::BlockStateId;
use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::translation;
use pumpkin_data::world::WorldEvent;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::bedrock::client::CUpdateBlock;
use pumpkin_protocol::java::client::play::CBlockUpdate;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::random::gameplay::game_random;
use pumpkin_util::text::TextComponent;
use pumpkin_world::generation::structure::template::PaletteEntry;
use pumpkin_world::world::BlockFlags;
use std::sync::atomic::{AtomicBool, AtomicI32};
use std::sync::{Arc, Mutex, atomic::Ordering};

use crate::{
    block::{
        blocks::{anvil::AnvilBlock, falling::FallingBlock},
        registry::can_replace_with_other_block,
    },
    entity::{Entity, EntityBase, living::LivingEntity},
    server::Server,
    world::World,
};

/// Vanilla `FallingBlockEntity.tick`: drop after this many ticks outside the build height.
const TIME_OUT_OF_WORLD: i32 = 100;
/// Vanilla `FallingBlockEntity.tick`: drop after this many ticks still falling.
const TIME_MAX: i32 = 600;
/// Vanilla `FallingBlockEntity.DEFAULT_MAX_FALL_DAMAGE`.
const DEFAULT_MAX_FALL_DAMAGE: i32 = 40;
/// Vanilla `AnvilBlock.falling` damage per block fallen.
const ANVIL_FALL_DAMAGE_PER_DISTANCE: f32 = 2.0;

pub struct FallingEntity {
    entity: Entity,
    block_state_id: AtomicCell<BlockStateId>,
    time: AtomicI32,
    drop_item: AtomicBool,
    /// Vanilla `cancelDrop`: land as nothing, neither block nor item.
    cancel_drop: AtomicBool,
    /// Vanilla `blockData`, saved as `TileEntityData`.
    block_data: Mutex<Option<NbtCompound>>,
    hurt_entities: AtomicBool,
    fall_damage_per_distance: AtomicCell<f32>,
    fall_damage_max: AtomicI32,
    fall_distance: AtomicCell<f64>,
}

impl FallingEntity {
    pub fn new(mut entity: Entity, block_state_id: BlockStateId) -> Self {
        entity.blocks_building = true;
        // Vanilla `FallingBlockEntity.defineSynchedData`.
        entity.synched_data.define(
            pumpkin_data::tracked_data::falling_block::DATA_START_POS,
            BlockPos::ZERO,
        );
        Self {
            entity,
            block_state_id: AtomicCell::new(block_state_id),
            time: AtomicI32::new(0),
            drop_item: AtomicBool::new(true),
            cancel_drop: AtomicBool::new(false),
            block_data: Mutex::new(None),
            hurt_entities: AtomicBool::new(false),
            fall_damage_per_distance: AtomicCell::new(0.0),
            fall_damage_max: AtomicI32::new(DEFAULT_MAX_FALL_DAMAGE),
            fall_distance: AtomicCell::new(0.0),
        }
    }

    /// Vanilla `FallingBlockEntity.disableDrop`.
    pub fn disable_drop(&self) {
        self.cancel_drop.store(true, Ordering::Relaxed);
    }

    /// Vanilla `FallingBlockEntity.setHurtsEntities`.
    pub fn set_hurts_entities(&self, damage_per_distance: f32, damage_max: i32) {
        self.hurt_entities.store(true, Ordering::Relaxed);
        self.fall_damage_per_distance.store(damage_per_distance);
        self.fall_damage_max.store(damage_max, Ordering::Relaxed);
    }

    /// Vanilla `Entity.checkFallDamage`. A falling block never runs the base tick, so it never
    /// counts as in water.
    fn check_fall_damage(&self, moved_y: f64) {
        if moved_y < 0.0 {
            self.fall_distance
                .store(self.fall_distance.load() - moved_y);
        }
        if self.entity.on_ground.load(Ordering::Relaxed) {
            let fall_distance = self.fall_distance.swap(0.0);
            if fall_distance > 0.0 {
                self.cause_fall_damage(fall_distance);
            }
        }
    }

    /// Vanilla `FallingBlockEntity.causeFallDamage`.
    fn cause_fall_damage(&self, fall_distance: f64) {
        if !self.hurt_entities.load(Ordering::Relaxed) {
            return;
        }
        let distance = (fall_distance - 1.0).ceil() as i32;
        if distance < 0 {
            return;
        }
        let state = self.block_state_id.load();
        let is_anvil = Block::from_state_id(state).has_tag(&tag::Block::MINECRAFT_ANVIL);
        // `Fallable.getFallDamageSource`: anvils have their own damage type.
        let damage_type = if is_anvil {
            DamageType::FALLING_ANVIL
        } else {
            DamageType::FALLING_BLOCK
        };
        let damage = ((distance as f32 * self.fall_damage_per_distance.load()).floor() as i32)
            .min(self.fall_damage_max.load(Ordering::Relaxed)) as f32;
        let world = self.entity.world.load();
        for target in world.get_entities_at_box(&self.entity.bounding_box.load()) {
            let target_entity = target.get_entity();
            // `NO_CREATIVE_OR_SPECTATOR` and `LIVING_ENTITY_STILL_ALIVE`.
            if target_entity.entity_id == self.entity.entity_id
                || target
                    .get_living_entity()
                    .is_none_or(|living| living.dead.load(Ordering::Relaxed))
                || target
                    .get_player()
                    .is_some_and(|player| player.is_creative() || player.is_spectator())
            {
                continue;
            }
            target.damage_with_context(
                target.as_ref(),
                damage,
                damage_type,
                None,
                Some(self),
                Some(self),
            );
        }
        if is_anvil && damage > 0.0 && game_random().next_f32() < 0.05 + distance as f32 * 0.05 {
            match AnvilBlock::damage(state) {
                Some(damaged) => self.block_state_id.store(damaged),
                None => self.disable_drop(),
            }
        }
    }

    /// Vanilla `Fallable.onLand`.
    fn on_land(&self, world: &World, pos: &BlockPos) {
        let block = Block::from_state_id(self.block_state_id.load());
        if block.has_tag(&tag::Block::MINECRAFT_ANVIL) && !self.entity.is_silent() {
            world.sync_world_event(WorldEvent::SoundAnvilLand, *pos, 0);
        }
    }

    /// Vanilla `Fallable.onBrokenAfterFall`.
    fn on_broken_after_fall(&self, world: &World, pos: &BlockPos) {
        let state = self.block_state_id.load();
        let block = Block::from_state_id(state);
        if block.has_tag(&tag::Block::MINECRAFT_ANVIL) {
            if !self.entity.is_silent() {
                world.sync_world_event(WorldEvent::SoundAnvilBroken, *pos, 0);
            }
        } else if block == &Block::SUSPICIOUS_SAND || block == &Block::SUSPICIOUS_GRAVEL {
            let bounding_box = self.entity.bounding_box.load();
            let center = bounding_box
                .min
                .add(&bounding_box.max)
                .multiply(0.5, 0.5, 0.5);
            world.sync_world_event(
                WorldEvent::ParticlesAndSoundDestroyBlock,
                BlockPos::floored_v(center),
                i32::from(state.as_u16()),
            );
            world.emit_game_event(
                pumpkin_data::game_event::GameEvent::BlockDestroy.name(),
                center,
            );
        }
    }

    /// Vanilla `callOnBrokenAfterFall` and `spawnAtLocation` when the block can't be placed.
    fn drop_as_item(&self, world: &World, pos: &BlockPos) {
        if self.drop_item.load(Ordering::Relaxed) && world.level_info.load().game_rules.entity_drops
        {
            self.on_broken_after_fall(world, pos);
            if let Some(item) =
                Item::from_id(Block::from_state_id(self.block_state_id.load()).item_id)
            {
                self.entity.spawn_at_location(ItemStack::new(1, item));
            }
        }
    }

    /// The landing branch of vanilla `FallingBlockEntity.tick` when the drop isn't cancelled.
    fn land(&self, world: &Arc<World>, pos: BlockPos, block: &Block, is_stuck_in_water: bool) {
        let mut state_id = self.block_state_id.load();
        let (current_block, current_state) = world.get_block_and_state(&pos);
        let (below_block, below_state) = world.get_block_and_state(&pos.down());
        let may_replace = can_replace_with_other_block(current_block, current_state);
        let would_continue_falling =
            FallingBlock::can_fall_through(below_state, below_block) && !is_stuck_in_water;
        let would_survive = world.block_registry.can_place_at(
            None,
            Some(&**world),
            &**world,
            None,
            block,
            state_id.to_state(),
            &pos,
            None,
            None,
        ) && !would_continue_falling;
        if !(may_replace && would_survive) {
            self.drop_as_item(world, &pos);
            return;
        }
        if world.is_water_source(&pos)
            && let Some(waterlogged) = state_id.to_state().set_waterlogged(true)
        {
            state_id = waterlogged.id;
            self.block_state_id.store(state_id);
        }
        // Vanilla `ConcretePowderBlock.onLand` turns the placed powder into concrete.
        if block.has_tag(&tag::Block::MINECRAFT_CONCRETE_POWDERS)
            && FallingBlock::should_solidify(&**world, &pos)
            && let Some(name) = block.name.strip_suffix("_powder")
            && let Some(concrete) = Block::from_name(name)
        {
            state_id = concrete.default_state.id;
        }
        world.set_block_state(&pos, state_id, BlockFlags::NOTIFY_ALL);
        // block updates to watchers before the despawn, else a invisible block gap until the tick flush.
        let placed = world.get_block_state_id(&pos);
        world.send_to_tracking_players_editioned(
            &self.entity,
            &CBlockUpdate::new(pos, i32::from(placed.as_u16()).into()),
            &CUpdateBlock::new(pos, BlockState::to_be_network_id(placed)),
        );
        self.on_land(world, &pos);
        self.apply_block_data(world, &pos);
    }

    /// Vanilla merges `blockData` over the landed block entity's saved fields.
    fn apply_block_data(&self, world: &Arc<World>, pos: &BlockPos) {
        let block_data = self
            .block_data
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let Some(block_data) = block_data else {
            return;
        };
        if self.block_state_id.load().to_state().block_entity_type == u16::MAX {
            return;
        }
        let Some(block_entity) = world.get_block_entity(pos) else {
            return;
        };
        let mut merged = NbtCompound::new();
        block_entity.write_internal(&mut merged);
        for (name, tag) in block_data.child_tags {
            merged.put(&name, tag);
        }
        world.load_block_entity_data(&block_entity, &merged);
        world.block_entity_set_changed(pos);
    }

    /// Replaces the current block with its fluid and spawns a falling one (synchronous)
    pub fn replace_spawn(
        world: &Arc<World>,
        position: BlockPos,
        block_state: BlockStateId,
    ) -> Arc<Self> {
        let replacement = if block_state.is_waterlogged() {
            Block::WATER.default_state.id
        } else {
            Block::AIR.default_state.id
        };
        world.set_block_state(&position, replacement, BlockFlags::NOTIFY_ALL);
        let block_state = block_state
            .to_state()
            .set_waterlogged(false)
            .map_or(block_state, |state| state.id);

        let position = position.0.to_f64().add_raw(0.5, 0.0, 0.5);
        let entity = Entity::new(world.clone(), position, &EntityType::FALLING_BLOCK);
        entity
            .data
            .store(i32::from(block_state.as_u16()), Ordering::Relaxed);
        // Vanilla `fall` sets the start position; a summoned falling block keeps the default.
        entity.set_synced_data(
            pumpkin_data::tracked_data::falling_block::DATA_START_POS,
            entity.block_pos.load(),
        );
        let entity = Arc::new(Self::new(entity, block_state));
        // Vanilla `FallingBlock.falling`, which only `AnvilBlock` overrides.
        if Block::from_state_id(block_state).has_tag(&tag::Block::MINECRAFT_ANVIL) {
            entity.set_hurts_entities(ANVIL_FALL_DAMAGE_PER_DISTANCE, DEFAULT_MAX_FALL_DAMAGE);
        }
        world.spawn_entity_non_save(entity.clone());
        entity
    }
}

impl EntityBase for FallingEntity {
    fn get_type_name(&self) -> TextComponent {
        let block_key = format!(
            "block.minecraft.{}",
            Block::from_state_id(self.block_state_id.load()).name
        );
        TextComponent::translate_cross(
            translation::java::ENTITY_MINECRAFT_FALLING_BLOCK_TYPE,
            translation::java::ENTITY_MINECRAFT_FALLING_BLOCK_TYPE,
            [TextComponent::translate_cross(
                block_key.clone(),
                block_key,
                [],
            )],
        )
    }

    fn tick(&self, caller: &dyn EntityBase, _server: &Server) {
        let entity = &self.entity;
        if self.block_state_id.load().to_state().is_air() {
            entity.remove();
            return;
        }
        let time = self.time.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        let mut velo = entity.velocity.load();
        velo.y -= self.get_gravity();

        entity.velocity.store(velo);

        let y_before = entity.pos.load().y;
        entity.move_entity(caller, velo);
        self.check_fall_damage(entity.pos.load().y - y_before);
        entity.tick_block_collisions(caller);
        let world = entity.world.load_full();
        let pos = self.entity.block_pos.load();
        let block = Block::from_state_id(self.block_state_id.load());
        // TODO: vanilla also clips through source water above a speed of 1 for concrete powder.
        let is_stuck_in_water = block.has_tag(&tag::Block::MINECRAFT_CONCRETE_POWDERS)
            && world.get_fluid(&pos).has_tag(&tag::Fluid::MINECRAFT_WATER);
        if !entity.on_ground.load(Ordering::Relaxed) && !is_stuck_in_water {
            let out_of_world = pos.0.y <= world.get_bottom_y() || pos.0.y > world.get_top_y();
            if (time > TIME_OUT_OF_WORLD && out_of_world) || time > TIME_MAX {
                if self.drop_item.load(Ordering::Relaxed)
                    && world.level_info.load().game_rules.entity_drops
                    && let Some(item) = Item::from_id(block.item_id)
                {
                    entity.spawn_at_location(ItemStack::new(1, item));
                }
                entity.remove();
            }
        } else {
            entity
                .velocity
                .store(entity.velocity.load().multiply(0.7, -0.5, 0.7));
            // Vanilla waits until a piston has finished moving a block through the landing cell.
            if world.get_block(&pos) != &Block::MOVING_PISTON {
                if self.cancel_drop.load(Ordering::Relaxed) {
                    self.on_broken_after_fall(&world, &pos);
                } else {
                    self.land(&world, pos, block, is_stuck_in_water);
                }
                entity.remove();
            }
        }

        let drag = f64::from(super::AIR_DRAG);
        entity
            .velocity
            .store(entity.velocity.load().multiply(drag, drag, drag));
    }

    fn write_custom_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put(
            "BlockState",
            PaletteEntry::state_to_nbt(self.block_state_id.load()),
        );
        nbt.put_int("Time", self.time.load(Ordering::Relaxed));
        nbt.put_bool("DropItem", self.drop_item.load(Ordering::Relaxed));
        nbt.put_bool("HurtEntities", self.hurt_entities.load(Ordering::Relaxed));
        nbt.put_float("FallHurtAmount", self.fall_damage_per_distance.load());
        nbt.put_int("FallHurtMax", self.fall_damage_max.load(Ordering::Relaxed));
        if let Some(block_data) = self
            .block_data
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            nbt.put_compound("TileEntityData", block_data.clone());
        }
        nbt.put_bool("CancelDrop", self.cancel_drop.load(Ordering::Relaxed));
    }

    fn read_custom_nbt(&self, nbt: &NbtCompound) {
        let state = nbt
            .get("BlockState")
            .and_then(PaletteEntry::state_from_nbt)
            .unwrap_or(Block::SAND.default_state.id);
        self.block_state_id.store(state);
        self.entity
            .data
            .store(i32::from(state.as_u16()), Ordering::Relaxed);
        self.time
            .store(nbt.get_int("Time").unwrap_or(0), Ordering::Relaxed);
        self.hurt_entities.store(
            nbt.get_bool("HurtEntities").unwrap_or_else(|| {
                Block::from_state_id(state).has_tag(&tag::Block::MINECRAFT_ANVIL)
            }),
            Ordering::Relaxed,
        );
        self.fall_damage_per_distance
            .store(nbt.get_float("FallHurtAmount").unwrap_or(0.0));
        self.fall_damage_max.store(
            nbt.get_int("FallHurtMax")
                .unwrap_or(DEFAULT_MAX_FALL_DAMAGE),
            Ordering::Relaxed,
        );
        self.drop_item
            .store(nbt.get_bool("DropItem").unwrap_or(true), Ordering::Relaxed);
        *self
            .block_data
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            nbt.get_compound("TileEntityData").cloned();
        self.cancel_drop.store(
            nbt.get_bool("CancelDrop").unwrap_or(false),
            Ordering::Relaxed,
        );
    }

    fn init_data_tracker(&self) {}

    fn get_entity(&self) -> &Entity {
        &self.entity
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        None
    }
    fn damage(&self, _caller: &dyn EntityBase, _amount: f32, _damage_type: DamageType) -> bool {
        false
    }

    /// Vanilla `FallingBlockEntity.isPickable`.
    fn can_hit(&self) -> bool {
        !self.entity.is_removed()
    }

    fn get_gravity(&self) -> f64 {
        0.04
    }

    fn entity_bedrock_spawn_metadata(
        &self,
    ) -> Option<pumpkin_protocol::bedrock::client::set_actor_data::SyncedActorDataList> {
        let mut meta =
            pumpkin_protocol::bedrock::client::set_actor_data::SyncedActorDataList::new();
        let runtime_id = BlockState::to_be_network_id(self.block_state_id.load());
        meta.set(
            pumpkin_protocol::bedrock::client::set_actor_data::entity_data_key::VARIANT,
            pumpkin_protocol::bedrock::client::set_actor_data::MetadataValue::Int(
                runtime_id as i32,
            ),
        );
        Some(meta)
    }

    fn bedrock_y_offset(&self) -> f64 {
        0.49
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }
}
