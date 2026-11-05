use std::sync::Arc;
use std::sync::atomic::Ordering;

use crossbeam::atomic::AtomicCell;
use pumpkin_data::block_properties::{Axis, PistonHeadLikeProperties, PistonType};
use pumpkin_data::{Block, BlockDirection, BlockState};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::{boundingbox::BoundingBox, position::BlockPos, vector3::Vector3};
use pumpkin_world::generation::structure::template::{BlockStateResolver, PaletteEntry};

use crate::block::blocks::piston::piston::PistonProps;
use crate::entity::EntityBase;
use crate::world::{BlockFlags, World};

use super::BlockEntity;

/// Vanilla `PistonMovingBlockEntity.PUSH_OFFSET`.
const PUSH_OFFSET: f64 = 0.01;
/// The top of the box above a honey block that `moveStuckEntities` carries entities in.
const STICKY_TOP_HEIGHT: f64 = 1.500_001_000_000_000_1;

pub struct PistonBlockEntity {
    pub position: BlockPos,
    pub pushed_block_state: &'static BlockState,
    pub facing: BlockDirection,
    pub current_progress: AtomicCell<f32>,
    pub last_progress: AtomicCell<f32>,
    pub last_ticked: AtomicCell<i64>,
    pub extending: bool,
    pub source: bool,
}

impl PistonBlockEntity {
    pub const ID: &'static str = "minecraft:piston";

    const fn movement_direction(&self) -> BlockDirection {
        if self.extending {
            self.facing
        } else {
            self.facing.opposite()
        }
    }

    /// Vanilla `getExtendedProgress`: how far back from the block's final position the
    /// moving block is at a given progress. Negative while extending.
    fn amount_extended(&self, progress: f32) -> f32 {
        if self.extending {
            progress - 1.0
        } else {
            1.0 - progress
        }
    }

    fn dir_vec(dir: BlockDirection, scale: f64) -> Vector3<f64> {
        let off = dir.to_offset();
        Vector3::new(
            f64::from(off.x) * scale,
            f64::from(off.y) * scale,
            f64::from(off.z) * scale,
        )
    }

    /// Vanilla `getCollisionRelatedBlockState`: a retracting piston collides as its head.
    fn collision_related_state(&self) -> &'static BlockState {
        let moved_block = Block::from_state_id(self.pushed_block_state.id);
        if self.extending
            || !self.source
            || (moved_block != &Block::PISTON && moved_block != &Block::STICKY_PISTON)
        {
            return self.pushed_block_state;
        }
        let mut props = PistonHeadLikeProperties::default(&Block::PISTON_HEAD);
        props.short = self.current_progress.load() > 0.25;
        props.r#type = if moved_block == &Block::STICKY_PISTON {
            PistonType::Sticky
        } else {
            PistonType::Normal
        };
        props.facing = PistonProps::from_state_id(self.pushed_block_state.id).facing;
        BlockState::from_id(props.to_state_id(&Block::PISTON_HEAD))
    }

    /// Vanilla `moveByPositionAndProgress`.
    fn move_by_position_and_progress(&self, aabb: BoundingBox) -> BoundingBox {
        let current = f64::from(self.amount_extended(self.current_progress.load()));
        aabb.shift(self.position.to_f64() + Self::dir_vec(self.facing, current))
    }

    /// Vanilla `PistonMath.getMovementArea`: the slab `amount` thick that the face of `aabb`
    /// sweeps through when it moves towards `dir`.
    fn movement_area(aabb: BoundingBox, dir: BlockDirection, amount: f64) -> BoundingBox {
        let off = dir.to_offset();
        let delta = amount * f64::from(off.x + off.y + off.z);
        let (min, max) = (delta.min(0.0), delta.max(0.0));
        let (mut from, mut to) = (aabb.min, aabb.max);
        match dir {
            BlockDirection::West => (from.x, to.x) = (aabb.min.x + min, aabb.min.x + max),
            BlockDirection::East => (from.x, to.x) = (aabb.max.x + min, aabb.max.x + max),
            BlockDirection::Down => (from.y, to.y) = (aabb.min.y + min, aabb.min.y + max),
            BlockDirection::Up => (from.y, to.y) = (aabb.max.y + min, aabb.max.y + max),
            BlockDirection::North => (from.z, to.z) = (aabb.min.z + min, aabb.min.z + max),
            BlockDirection::South => (from.z, to.z) = (aabb.max.z + min, aabb.max.z + max),
        }
        BoundingBox::new(from, to)
    }

    /// Vanilla `getMovement`: how far `aabb` has to move along `dir` to leave `outside_of`.
    fn get_movement(outside_of: BoundingBox, dir: BlockDirection, aabb: BoundingBox) -> f64 {
        match dir {
            BlockDirection::East => outside_of.max.x - aabb.min.x,
            BlockDirection::West => aabb.max.x - outside_of.min.x,
            BlockDirection::Up => outside_of.max.y - aabb.min.y,
            BlockDirection::Down => aabb.max.y - outside_of.min.y,
            BlockDirection::South => outside_of.max.z - aabb.min.z,
            BlockDirection::North => aabb.max.z - outside_of.min.z,
        }
    }

    /// Vanilla `moveCollidedEntities`. Players aren't in the world's entity list: their
    /// clients run the same push and slime launch for themselves.
    fn move_collided_entities(&self, world: &Arc<World>, new_progress: f32) {
        let movement = self.movement_direction();
        let delta_progress = f64::from(new_progress - self.current_progress.load());
        let shapes: Vec<BoundingBox> = self
            .collision_related_state()
            .get_block_collision_shapes()
            .collect();
        let Some(bounds) = shapes.iter().copied().reduce(|a, b| a.union(&b)) else {
            return;
        };
        let aabb = self.move_by_position_and_progress(bounds);
        let area = Self::movement_area(aabb, movement, delta_progress).union(&aabb);
        let entities = world.get_entities_at_box(&area);
        if entities.is_empty() {
            return;
        }
        let cause_bounce = Block::from_state_id(self.pushed_block_state.id) == &Block::SLIME_BLOCK;

        for entity in entities {
            if entity.is_ignored_by_pistons() {
                continue;
            }
            let base = entity.get_entity();
            // Vanilla `canSimulateMovement`: a player controlled vehicle moves on its client.
            if cause_bounce
                && entity
                    .get_controlling_passenger()
                    .is_none_or(|passenger| passenger.get_player().is_none())
            {
                let mut velocity = base.velocity.load();
                let step = movement.to_offset();
                match movement.to_axis() {
                    Axis::X => velocity.x = f64::from(step.x),
                    Axis::Y => velocity.y = f64::from(step.y),
                    Axis::Z => velocity.z = f64::from(step.z),
                }
                base.velocity.store(velocity);
            }

            let mut delta: f64 = 0.0;
            for shape in &shapes {
                let moving = Self::movement_area(
                    self.move_by_position_and_progress(*shape),
                    movement,
                    delta_progress,
                );
                let entity_aabb = base.bounding_box.load();
                if moving.intersects(&entity_aabb) {
                    delta = delta.max(Self::get_movement(moving, movement, entity_aabb));
                    if delta >= delta_progress {
                        break;
                    }
                }
            }
            if delta <= 0.0 {
                continue;
            }
            let delta = delta.min(delta_progress) + PUSH_OFFSET;
            base.move_by_piston(entity.as_ref(), Self::dir_vec(movement, delta));
            if !self.extending && self.source {
                self.fix_entity_within_piston_base(entity.as_ref(), movement, delta_progress);
            }
        }
    }

    /// Vanilla `fixEntityWithinPistonBase`: a retracting head pushes an entity it pulled into
    /// the piston's own block back out.
    fn fix_entity_within_piston_base(
        &self,
        entity: &dyn EntityBase,
        movement: BlockDirection,
        delta_progress: f64,
    ) {
        let base = entity.get_entity();
        let entity_aabb = base.bounding_box.load();
        let block = BoundingBox::from_block(&self.position);
        if !entity_aabb.intersects(&block) {
            return;
        }
        let opposite = movement.opposite();
        let delta = Self::get_movement(block, opposite, entity_aabb) + PUSH_OFFSET;
        let delta_intersected =
            Self::get_movement(block, opposite, entity_aabb.intersection(&block)) + PUSH_OFFSET;
        if (delta - delta_intersected).abs() < PUSH_OFFSET {
            let delta = delta.min(delta_progress) + PUSH_OFFSET;
            base.move_by_piston(entity, Self::dir_vec(opposite, delta));
        }
    }

    /// Vanilla `moveStuckEntities`: a honey block moved sideways carries what stands on it.
    fn move_stuck_entities(&self, world: &Arc<World>, new_progress: f32) {
        if Block::from_state_id(self.pushed_block_state.id) != &Block::HONEY_BLOCK {
            return;
        }
        let movement = self.movement_direction();
        if movement.to_axis() == Axis::Y {
            return;
        }
        let sticky_top = self
            .pushed_block_state
            .get_block_collision_shapes()
            .map(|shape| shape.max.y)
            .fold(f64::NEG_INFINITY, f64::max);
        let aabb = self.move_by_position_and_progress(BoundingBox::new(
            Vector3::new(0.0, sticky_top, 0.0),
            Vector3::new(1.0, STICKY_TOP_HEIGHT, 1.0),
        ));
        let delta_progress = f64::from(new_progress - self.current_progress.load());
        for entity in world.get_entities_at_box(&aabb) {
            let base = entity.get_entity();
            let pos = base.pos.load();
            // Vanilla `matchesStickyCritera`.
            if entity.is_ignored_by_pistons()
                || !base.on_ground.load(Ordering::Relaxed)
                || !(base.supporting_block_pos.load() == Some(self.position)
                    || (pos.x >= aabb.min.x
                        && pos.x <= aabb.max.x
                        && pos.z >= aabb.min.z
                        && pos.z <= aabb.max.z))
            {
                continue;
            }
            base.move_by_piston(entity.as_ref(), Self::dir_vec(movement, delta_progress));
        }
    }

    pub fn finish(&self, world: &Arc<World>) {
        if self.last_progress.load() < 1.0 {
            self.current_progress.store(1.0);
            self.last_progress.store(1.0);
            let pos = self.position;
            world.remove_block_entity(&pos);
            if world.get_block(&pos) == &Block::MOVING_PISTON {
                let state = if self.source {
                    Block::AIR.default_state.id
                } else {
                    world.update_from_neighbor_shapes(self.pushed_block_state.id, &pos)
                };
                world.set_block_state(&pos, state, BlockFlags::NOTIFY_ALL);
                world.update_neighbor(&pos, Block::from_state_id(state));
            }
        }
    }
}

const BLOCK_STATE: &str = "blockState";
const FACING: &str = "facing";
const LAST_PROGRESS: &str = "progress";
const EXTENDING: &str = "extending";
const SOURCE: &str = "source";

impl BlockEntity for PistonBlockEntity {
    fn resource_location(&self) -> &'static str {
        Self::ID
    }

    fn get_position(&self) -> BlockPos {
        self.position
    }

    fn tick(&self, world: &Arc<World>) {
        self.last_ticked.store(world.get_world_age());
        let current_progress = self.current_progress.load();
        self.last_progress.store(current_progress);
        if current_progress >= 1.0 {
            let pos = self.position;
            world.remove_block_entity(&pos);
            if world.get_block(&pos) == &Block::MOVING_PISTON {
                let new_state = world.update_from_neighbor_shapes(self.pushed_block_state.id, &pos);
                if new_state.to_state().is_air() {
                    // The moved block can't survive here, so it is placed and then broken with drops.
                    world.set_block_state(
                        &pos,
                        self.pushed_block_state.id,
                        BlockFlags::SKIP_BLOCK_ENTITY_REPLACED_CALLBACK
                            | BlockFlags::MOVED
                            | BlockFlags::UPDATE_KNOWN_SHAPE,
                    );
                    world.break_block(&pos, None, BlockFlags::NOTIFY_ALL);
                } else {
                    let new_block = Block::from_state_id(new_state);
                    let new_state = new_block
                        .set_waterlogged(new_state, false)
                        .unwrap_or(new_state);
                    world.set_block_state(
                        &pos,
                        new_state,
                        BlockFlags::NOTIFY_ALL | BlockFlags::MOVED,
                    );
                    world.update_neighbor(&pos, new_block);
                }
            }
            return;
        }
        let new_progress = current_progress + 0.5;
        self.move_collided_entities(world, new_progress);
        self.move_stuck_entities(world, new_progress);
        self.current_progress.store(new_progress.min(1.0));
    }

    fn from_nbt(nbt: &pumpkin_nbt::compound::NbtCompound, position: BlockPos) -> Self
    where
        Self: Sized,
    {
        let pushed_block_state = nbt
            .get(BLOCK_STATE)
            .and_then(PaletteEntry::from_nbt_tag)
            .and_then(|entry| BlockStateResolver::resolve_simple(&entry))
            .unwrap_or(Block::AIR.default_state);
        let facing = nbt.get_byte(FACING).unwrap_or(0);
        let last_progress = nbt.get_float(LAST_PROGRESS).unwrap_or(0.0);
        let extending = nbt.get_bool(EXTENDING).unwrap_or(false);
        let source = nbt.get_bool(SOURCE).unwrap_or(false);
        Self {
            pushed_block_state,
            position,
            facing: BlockDirection::from_index(facing as u8).unwrap_or(BlockDirection::Down),
            current_progress: last_progress.into(),
            last_progress: last_progress.into(),
            last_ticked: 0.into(),
            extending,
            source,
        }
    }

    fn write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put(
            BLOCK_STATE,
            PaletteEntry::state_to_nbt_tag(self.pushed_block_state.id),
        );
        nbt.put_byte(FACING, self.facing.to_index() as i8);
        nbt.put_float(LAST_PROGRESS, self.last_progress.load());
        nbt.put_bool(EXTENDING, self.extending);
        nbt.put_bool(SOURCE, self.source);
    }

    fn chunk_data_nbt(&self) -> Option<NbtCompound> {
        let mut nbt = NbtCompound::new();
        self.write_nbt(&mut nbt);
        Some(nbt)
    }

    // The client animates moving blocks itself from the piston block event.
    fn update_packet_nbt(&self, _state_id: pumpkin_data::BlockStateId) -> Option<NbtCompound> {
        None
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
