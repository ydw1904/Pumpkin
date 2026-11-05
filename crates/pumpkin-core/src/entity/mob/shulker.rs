use pumpkin_util::random::gameplay::game_random;
use std::sync::atomic::{AtomicI8, AtomicI32, AtomicU8, Ordering};
use std::sync::{Arc, Weak};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::EntityType;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::world::WorldEvent;
use pumpkin_data::{Block, BlockDirection};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::java::client::play::CEntityPositionSync;
use pumpkin_util::Difficulty;
use pumpkin_util::math::boundingbox::{BoundingBox, EntityDimensions};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::generation::update_shape::is_face_full;
use uuid::Uuid;

use crate::entity::ai::behavior::hunting::{self, Hunt};
use crate::entity::ai::goal::active_target::TargetCondition;
use crate::entity::ai::goal::look_around::RandomLookAroundGoal;
use crate::entity::ai::goal::look_at_entity::LookAtEntityGoal;
use crate::entity::ai::goal::revenge::RevengeGoal;
use crate::entity::ai::goal::{Controls, Goal, to_goal_ticks};
use crate::entity::ai::target_match::TargetMatch;
use crate::entity::attributes::{Modifier, ModifierOperation, send_attribute_updates_for_living};
use crate::entity::mob::{Mob, MobEntity};
use crate::entity::projectile::is_abstract_arrow;
use crate::entity::projectile::shulker_bullet::ShulkerBulletEntity;
use crate::entity::r#type::from_type;
use crate::entity::{Entity, EntityBase};
use crate::world::game_event::GameEventContext;

const DEFAULT_ATTACH_FACE: BlockDirection = BlockDirection::Down;
const NO_COLOR: u8 = 16;
const PEEK_PER_TICK: f32 = 0.05;
const MAX_TELEPORT_DISTANCE: i32 = 8;
const TELEPORT_ATTEMPTS: usize = 5;
const OTHER_SHULKER_SCAN_RADIUS: f64 = 8.0;
const OTHER_SHULKER_LIMIT: f32 = 5.0;
const MAX_SCALE: f32 = 3.0;
const COVERED_ARMOR_MODIFIER_ID: &str = "minecraft:covered";
const COVERED_ARMOR: f64 = 20.0;
const SEARCH_FLAT_RANGE: f64 = 4.0;
/// Vanilla `ShulkerAttackGoal`: bullets only fly at targets closer than 20 blocks.
const ATTACK_RANGE_SQ: f64 = 400.0;

/// Vanilla `Shulker.getProgressAabb`: the shell of `size` at `position`, opened by `progress`
/// towards `direction`.
fn progress_aabb(
    size: f64,
    direction: BlockDirection,
    progress: f64,
    position: Vector3<f64>,
) -> BoundingBox {
    progress_delta_aabb(size, direction, -1.0, progress, position)
}

/// Vanilla `Shulker.getProgressDeltaAabb`: the space the shell sweeps opening from
/// `progress_from` to `progress_to`.
fn progress_delta_aabb(
    size: f64,
    direction: BlockDirection,
    progress_from: f64,
    progress_to: f64,
    position: Vector3<f64>,
) -> BoundingBox {
    let step = direction.to_offset().to_f64();
    let max_movement = progress_from.max(progress_to) * size;
    let min_movement = (1.0 + progress_from.min(progress_to)) * size;
    BoundingBox::new(
        Vector3::new(-size * 0.5, 0.0, -size * 0.5),
        Vector3::new(size * 0.5, size, size * 0.5),
    )
    .expand_towards(
        step.x * max_movement,
        step.y * max_movement,
        step.z * max_movement,
    )
    .contract(
        -step.x * min_movement,
        -step.y * min_movement,
        -step.z * min_movement,
    )
    .shift(position)
}

/// Vanilla `Mth.clamp`, which doesn't panic when the raw peek puts `min` above `max`.
fn mth_clamp(value: f32, min: f32, max: f32) -> f32 {
    if value < min { min } else { value.min(max) }
}

/// Vanilla `Shulker.getPhysicalPeek`.
fn physical_peek(amount: f32) -> f32 {
    0.5 - pumpkin_util::math::sin((0.5 + amount) * std::f32::consts::PI) * 0.5
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    X = 0,
    Y = 1,
    Z = 2,
}

trait BlockDirectionExt {
    fn axis_of(self) -> Axis;
}

impl BlockDirectionExt for BlockDirection {
    fn axis_of(self) -> Axis {
        match self {
            Self::East | Self::West => Axis::X,
            Self::Up | Self::Down => Axis::Y,
            Self::North | Self::South => Axis::Z,
        }
    }
}

/// Vanilla `ShulkerNearestAttackGoal` and `ShulkerDefenseAttackGoal`. Players are searched
/// without a box, so only the defense goal sees the flattened one.
const SHULKER_HUNTS: &[Hunt] = &[
    Hunt::new(2, TargetMatch::Type(&EntityType::PLAYER), true)
        .gated_by(not_peaceful)
        .searching_in(shulker_target_search_area),
    Hunt::new(3, TargetMatch::Enemy, true)
        .when(TargetCondition::OnTeam)
        .searching_in(shulker_target_search_area),
];

fn is_peaceful(mob: &dyn Mob) -> bool {
    mob.get_entity().world.load().level_info.load().difficulty == Difficulty::Peaceful
}

fn not_peaceful(mob: &dyn Mob) -> bool {
    !is_peaceful(mob)
}

/// Vanilla `getTargetSearchArea` of both shulker target goals: only 4 blocks along the
/// attach face's axis.
fn shulker_target_search_area(mob: &dyn Mob, follow_range: f64) -> BoundingBox {
    let face = mob
        .cast_any()
        .downcast_ref::<ShulkerEntity>()
        .map_or(DEFAULT_ATTACH_FACE, ShulkerEntity::get_attach_face);
    let bounding_box = mob.get_entity().bounding_box.load();
    match face.axis_of() {
        Axis::X => bounding_box.expand(SEARCH_FLAT_RANGE, follow_range, follow_range),
        Axis::Z => bounding_box.expand(follow_range, follow_range, SEARCH_FLAT_RANGE),
        Axis::Y => bounding_box.expand(follow_range, SEARCH_FLAT_RANGE, follow_range),
    }
}

pub struct ShulkerEntity {
    pub mob_entity: MobEntity,
    attach_face: AtomicU8,
    /// Vanilla `DATA_PEEK_ID`, a signed byte.
    peek_amount: AtomicI8,
    color: AtomicU8,
    current_peek_amount: AtomicCell<f32>,
    prev_peek_amount: AtomicCell<f32>,
}

impl ShulkerEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        // Vanilla `Shulker.defineSynchedData`.
        entity.synched_data.define(
            pumpkin_data::tracked_data::shulker::DATA_ATTACH_FACE_ID,
            VarInt(DEFAULT_ATTACH_FACE as i32),
        );
        entity
            .synched_data
            .define(pumpkin_data::tracked_data::shulker::DATA_PEEK_ID, 0i8);
        entity
            .synched_data
            .define(pumpkin_data::tracked_data::shulker::DATA_COLOR_ID, NO_COLOR);
        let mob_entity = MobEntity::new(entity);
        let shulker = Self {
            mob_entity,
            attach_face: AtomicU8::new(DEFAULT_ATTACH_FACE as u8),
            peek_amount: AtomicI8::new(0),
            color: AtomicU8::new(NO_COLOR),
            current_peek_amount: AtomicCell::new(0.0),
            prev_peek_amount: AtomicCell::new(0.0),
        };
        let mob_arc = Arc::new(shulker);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        {
            let goal_selector = &mob_arc.mob_entity.goals_selector;
            let target_selector = &mob_arc.mob_entity.target_selector;

            goal_selector.add_goal(
                1,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 8.0),
            );
            goal_selector.add_goal(4, Box::new(ShulkerAttackGoal::new(mob_arc.clone())));
            goal_selector.add_goal(7, Box::new(ShulkerPeekGoal::new(mob_arc.clone())));
            goal_selector.add_goal(8, Box::new(RandomLookAroundGoal::default()));

            target_selector.add_goal(
                1,
                Box::new(
                    RevengeGoal::new(true)
                        .ignoring(|entity_type| entity_type == &EntityType::SHULKER)
                        .alerting_others(),
                ),
            );
            hunting::apply(target_selector, &mob_arc.mob_entity, SHULKER_HUNTS);
        };

        mob_arc
    }

    pub fn get_attach_face(&self) -> BlockDirection {
        BlockDirection::try_from(i32::from(self.attach_face.load(Ordering::Relaxed)))
            .unwrap_or(DEFAULT_ATTACH_FACE)
    }

    pub fn set_attach_face(&self, face: BlockDirection) {
        self.attach_face.store(face as u8, Ordering::Relaxed);
        let entity = &self.mob_entity.living_entity.entity;
        entity.set_synced_data(
            pumpkin_data::tracked_data::shulker::ATTACH_FACE_ID,
            VarInt(face as i32),
        );
        // `onSyncedDataUpdated`.
        self.update_bounding_box();
    }

    /// Vanilla `getScale` with `Shulker.sanitizeScale`.
    fn scale(&self) -> f32 {
        (self
            .mob_entity
            .living_entity
            .get_attribute_value(&Attributes::SCALE) as f32)
            .min(MAX_SCALE)
    }

    /// Vanilla `Shulker.makeBoundingBox`: the shell opened by its physical peek.
    fn update_bounding_box(&self) {
        let peek = physical_peek(self.current_peek_amount.load());
        self.mob_entity
            .living_entity
            .entity
            .set_relative_bounding_box(progress_aabb(
                f64::from(self.scale()),
                self.get_attach_face().opposite(),
                f64::from(peek),
                Vector3::default(),
            ));
    }

    /// Vanilla `Shulker.getDefaultDimensions`: a shell on the floor grows taller as it opens.
    fn refresh_dimensions(&self) {
        let entity = &self.mob_entity.living_entity.entity;
        let peek = self.current_peek_amount.load();
        if self.get_attach_face() == BlockDirection::Down && peek > 0.0 {
            let base = Entity::type_dimensions(entity.entity_type);
            let height_scale = 1.0 + peek;
            entity.set_default_dimensions(EntityDimensions::new(
                base.width,
                base.height * height_scale,
                base.eye_height * height_scale,
            ));
        } else {
            entity.clear_default_dimensions();
        }
        // `Shulker.setPos` keeps it on the block centre wherever
        // `fudgePositionAfterSizeChange` puts it.
        if !entity.has_vehicle() {
            let pos = entity.pos.load();
            let centred = Vector3::new(
                pos.x.floor() + 0.5,
                (pos.y + 0.5).floor(),
                pos.z.floor() + 0.5,
            );
            if centred != pos {
                entity.set_pos(centred);
            }
        }
    }

    /// Vanilla `Shulker.onPeekAmountChange`: an opening shell pushes what is in its way.
    fn on_peek_amount_change(&self) {
        let entity = &self.mob_entity.living_entity.entity;
        // `reapplyPosition`.
        self.update_bounding_box();
        self.refresh_dimensions();
        let scale = self.scale();
        let peek = physical_peek(self.current_peek_amount.load());
        let peek_old = physical_peek(self.prev_peek_amount.load());
        let push = (peek - peek_old) * scale;
        if push <= 0.0 {
            return;
        }
        let direction = self.get_attach_face().opposite();
        let area = progress_delta_aabb(
            f64::from(scale),
            direction,
            f64::from(peek_old),
            f64::from(peek),
            entity.pos.load(),
        );
        let motion = direction.to_offset().to_f64() * f64::from(push);
        // Players push themselves: the client runs this for its own shulkers too.
        for other in entity.world.load().get_entities_at_box(&area) {
            let other_entity = other.get_entity();
            if other_entity.entity_id == entity.entity_id
                || other_entity.entity_type == &EntityType::SHULKER
                || other_entity.no_physics.load(Ordering::Relaxed)
                || other_entity.is_passenger_of_same_vehicle(entity)
            {
                continue;
            }
            other.move_entity(other.as_ref(), motion);
        }
    }

    pub fn get_color(&self) -> Option<u8> {
        let c = self.color.load(Ordering::Relaxed);
        if c == NO_COLOR { None } else { Some(c) }
    }

    pub fn set_color(&self, color: Option<u8>) {
        let val = color.unwrap_or(NO_COLOR);
        self.color.store(val, Ordering::Relaxed);
        let entity = &self.mob_entity.living_entity.entity;
        entity.set_synced_data(pumpkin_data::tracked_data::shulker::COLOR, val as i8);
    }

    pub fn get_raw_peek(&self) -> i8 {
        self.peek_amount.load(Ordering::Relaxed)
    }

    /// Vanilla `Shulker.setRawPeekAmount`: closing adds the covered armor, every change plays
    /// its sound.
    pub fn set_raw_peek(&self, amount: i8) {
        let living = &self.mob_entity.living_entity;
        living.update_attribute(&Attributes::ARMOR, |armor| {
            armor.remove_modifier(COVERED_ARMOR_MODIFIER_ID);
            if amount == 0 {
                armor.add_or_replace_permanent_modifier(Modifier {
                    id: COVERED_ARMOR_MODIFIER_ID.to_string(),
                    amount: COVERED_ARMOR,
                    operation: ModifierOperation::Add,
                });
            }
        });
        send_attribute_updates_for_living(living, vec![Attributes::ARMOR]);
        let (sound, event) = if amount == 0 {
            (Sound::EntityShulkerClose, GameEvent::ContainerClose)
        } else {
            (Sound::EntityShulkerOpen, GameEvent::ContainerOpen)
        };
        self.play_entity_sound(sound, 1.0, 1.0);
        let entity = &living.entity;
        entity.world.load().emit_game_event_with_context(
            event.name(),
            entity.pos.load(),
            &GameEventContext {
                source_entity: Some(self),
            },
        );
        self.store_raw_peek(amount);
    }

    fn store_raw_peek(&self, amount: i8) {
        self.peek_amount.store(amount, Ordering::Relaxed);
        self.mob_entity
            .living_entity
            .entity
            .set_synced_data(pumpkin_data::tracked_data::shulker::PEEK_ID, amount);
    }

    pub fn is_closed(&self) -> bool {
        self.peek_amount.load(Ordering::Relaxed) == 0
    }

    /// Vanilla `Shulker.updatePeekAmount`: the shell moves 0.05 a tick towards the raw peek.
    fn update_peek_amount(&self) -> bool {
        let current = self.current_peek_amount.load();
        self.prev_peek_amount.store(current);
        let target = f32::from(self.get_raw_peek()) * 0.01;

        if current == target {
            return false;
        }

        let new = if current > target {
            mth_clamp(current - PEEK_PER_TICK, target, 1.0)
        } else {
            mth_clamp(current + PEEK_PER_TICK, 0.0, target)
        };

        self.current_peek_amount.store(new);
        true
    }

    /// Vanilla `Shulker.findAttachableSurface`.
    fn find_attachable_face(&self, pos: &BlockPos) -> Option<BlockDirection> {
        BlockDirection::all()
            .into_iter()
            .find(|&face| self.can_stay_at(pos, face))
    }

    /// Vanilla `Shulker.canStayAt`: `pos` is free, the block towards `face` has a full face
    /// towards it and the fully opened shell fits.
    fn can_stay_at(&self, pos: &BlockPos, face: BlockDirection) -> bool {
        if self.is_position_blocked(pos) {
            return false;
        }
        let living = &self.mob_entity.living_entity;
        let world = living.entity.world.load();
        let opposite = face.opposite();
        // `loadedAndEntityCanStandOnFace`: unloaded blocks read as air.
        let support = pos.offset_direction(face);
        if !is_face_full(world.get_block_state_id(&support), support.0, opposite) {
            return false;
        }
        let scale = f64::from(living.get_attribute_value(&Attributes::SCALE) as f32);
        let fully_opened = progress_aabb(
            scale,
            opposite,
            1.0,
            Vector3::new(
                f64::from(pos.0.x) + 0.5,
                f64::from(pos.0.y),
                f64::from(pos.0.z) + 0.5,
            ),
        )
        .expand_all(-1.0E-6);
        world.no_collision(self, &fully_opened)
    }

    /// Vanilla `Shulker.isPositionBlocked`: anything but air, except a moving piston in its
    /// own block.
    fn is_position_blocked(&self, pos: &BlockPos) -> bool {
        let entity = &self.mob_entity.living_entity.entity;
        let state = entity.world.load().get_block_state(pos);
        !state.is_air()
            && (Block::from_state_id(state.id) != &Block::MOVING_PISTON
                || *pos != entity.block_pos.load())
    }

    fn find_new_attachment(&self) {
        let pos = self.mob_entity.living_entity.entity.block_pos.load();
        if let Some(dir) = self.find_attachable_face(&pos) {
            self.set_attach_face(dir);
        } else {
            self.teleport_somewhere();
        }
    }

    /// Vanilla `Shulker.teleportSomewhere`.
    pub fn teleport_somewhere(&self) -> bool {
        let entity = &self.mob_entity.living_entity.entity;
        if self.mob_entity.is_no_ai() || !entity.is_alive() {
            return false;
        }
        let old_pos = entity.block_pos.load();
        let world = entity.world.load();

        for _ in 0..TELEPORT_ATTEMPTS {
            let offset = {
                let rng = self.get_random();
                Vector3::new(
                    rng.random_range(-MAX_TELEPORT_DISTANCE..=MAX_TELEPORT_DISTANCE),
                    rng.random_range(-MAX_TELEPORT_DISTANCE..=MAX_TELEPORT_DISTANCE),
                    rng.random_range(-MAX_TELEPORT_DISTANCE..=MAX_TELEPORT_DISTANCE),
                )
            };
            let target = old_pos.offset(offset);
            let within_border = world
                .worldborder
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains(f64::from(target.0.x), f64::from(target.0.z));
            if target.0.y <= world.get_bottom_y()
                || !world.get_block_state(&target).is_air()
                || !within_border
                || !world.no_collision(self, &BoundingBox::from_block(&target).expand_all(-1.0E-6))
            {
                continue;
            }
            let Some(face) = self.find_attachable_face(&target) else {
                continue;
            };
            if world
                .get_block(&target.offset_direction(face))
                .has_tag(&tag::Block::MINECRAFT_SHULKER_DOES_NOT_TELEPORT_TO)
            {
                continue;
            }

            // `unRide`.
            if let Some(vehicle) = entity.get_vehicle() {
                vehicle.get_entity().remove_passenger(entity.entity_id);
            }
            entity.eject_passengers();
            self.set_attach_face(face);
            // Played where it was, before it moves.
            self.play_entity_sound(Sound::EntityShulkerTeleport, 1.0, 1.0);
            let new_pos = Vector3::new(
                f64::from(target.0.x) + 0.5,
                f64::from(target.0.y),
                f64::from(target.0.z) + 0.5,
            );
            entity.set_pos(new_pos);
            world.broadcast_to_chunk(
                entity.chunk_pos.load(),
                &CEntityPositionSync::new(
                    entity.entity_id.into(),
                    new_pos,
                    Vector3::new(0.0, 0.0, 0.0),
                    entity.yaw.load(),
                    entity.pitch.load(),
                    entity.on_ground.load(Ordering::Relaxed),
                ),
            );
            entity.last_sent_pos.store(new_pos);
            world.emit_game_event_with_context(
                GameEvent::Teleport.name(),
                old_pos.to_centered_f64(),
                &GameEventContext {
                    source_entity: Some(self),
                },
            );
            world.sync_world_event(
                WorldEvent::ParticlesShulkerTeleport,
                old_pos,
                old_pos.pack_difference_in_position(
                    &entity.block_pos.load(),
                    MAX_TELEPORT_DISTANCE,
                    MAX_TELEPORT_DISTANCE,
                    MAX_TELEPORT_DISTANCE,
                ),
            );
            // Unlike `set_raw_peek`: no sound and no armor change.
            self.store_raw_peek(0);
            self.mob_entity.set_target(None);
            return true;
        }
        false
    }

    /// The end of vanilla `Shulker.hurtServer`, after the damage landed.
    fn on_shulker_damage(&self, damage_type: DamageType, source: Option<&dyn EntityBase>) {
        let living = &self.mob_entity.living_entity;
        if living.health.load() < living.get_max_health() * 0.5
            && self.get_random().random_range(0..4) == 0
        {
            self.teleport_somewhere();
        } else if damage_type.has_tag(&tag::DamageType::MINECRAFT_IS_PROJECTILE)
            && source.is_some_and(|source| {
                source.get_entity().entity_type == &EntityType::SHULKER_BULLET
            })
        {
            self.hit_by_shulker_bullet();
        }
    }

    /// Vanilla `Shulker.hitByShulkerBullet`: an open shulker teleports away and leaves a copy
    /// behind, less often the more shulkers are around.
    fn hit_by_shulker_bullet(&self) {
        let entity = &self.mob_entity.living_entity.entity;
        let old_position = entity.pos.load();
        let old_box = entity.bounding_box.load();
        if self.is_closed() || !self.teleport_somewhere() {
            return;
        }
        let world = entity.world.load();
        let shulker_count = world
            .get_entities_of_types_at_box(&old_box.expand_all(OTHER_SHULKER_SCAN_RADIUS), |kind| {
                kind == &EntityType::SHULKER
            })
            .iter()
            .filter(|shulker| shulker.get_entity().is_alive())
            .count();
        let failure_chance = (shulker_count as f32 - 1.0) / OTHER_SHULKER_LIMIT;
        if game_random().random::<f32>() < failure_chance {
            return;
        }
        let baby = from_type(&EntityType::SHULKER, old_position, &world, Uuid::new_v4());
        if let Some(shulker) = baby.cast_any().downcast_ref::<Self>() {
            shulker.set_color(self.get_color());
        }
        world.spawn_entity(baby);
    }
}

impl Mob for ShulkerEntity {
    fn play_ambient_sound(&self) {
        if !self.is_closed() {
            self.make_sound(Sound::EntityShulkerAmbient);
        }
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_byte("AttachFace", self.attach_face.load(Ordering::Relaxed) as i8);
        nbt.put_byte("Peek", self.peek_amount.load(Ordering::Relaxed));
        nbt.put_byte("Color", self.color.load(Ordering::Relaxed) as i8);
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        // `Direction.from3DDataValue`.
        let face = nbt
            .get_byte("AttachFace")
            .and_then(|face| BlockDirection::try_from((i32::from(face) % 6).abs()).ok())
            .unwrap_or(DEFAULT_ATTACH_FACE);
        self.set_attach_face(face);
        self.store_raw_peek(nbt.get_byte("Peek").unwrap_or(0));
        self.set_color(Some(
            nbt.get_byte("Color").map_or(NO_COLOR, |color| color as u8),
        ));
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn get_mob_gravity(&self) -> f64 {
        0.0
    }

    fn mob_tick(&self, _caller: &dyn EntityBase) {
        let entity = &self.mob_entity.living_entity.entity;

        if !entity.is_alive() {
            return;
        }

        entity.velocity.store(Vector3::new(0.0, 0.0, 0.0));
    }

    // The rest of vanilla `Shulker.tick`, after `super.tick()`.
    fn post_tick(&self) {
        let entity = &self.mob_entity.living_entity.entity;
        let pos = entity.block_pos.load();
        if !entity.has_vehicle() && !self.can_stay_at(&pos, self.get_attach_face()) {
            self.find_new_attachment();
        }
        if self.update_peek_amount() {
            self.on_peek_amount_change();
        }
    }

    /// Vanilla `Shulker.setPos`: carried to another block after its first tick, it closes
    /// without the sound or the armor change. `/tp` uses `snapTo`, which keeps the peek.
    fn mob_on_positioned_on_vehicle(&self, old_block_pos: BlockPos) {
        let entity = &self.mob_entity.living_entity.entity;
        if entity.tick_count.load(Ordering::Relaxed) != 0
            && entity.block_pos.load() != old_block_pos
        {
            self.store_raw_peek(0);
        }
    }

    fn on_damage(&self, damage_type: DamageType, source: Option<&dyn EntityBase>) {
        self.on_shulker_damage(damage_type, source);
    }

    // `on_shulker_damage` can teleport it to a block around it, checking the blocks and
    // collidable entities there and its fully opened shell one block further, then count the
    // shulkers within `OTHER_SHULKER_SCAN_RADIUS` of its old box and leave a copy there.
    fn hurt_reach(&self) -> Option<crate::world::entity_islands::HurtReach> {
        let reach = f64::from(MAX_TELEPORT_DISTANCE) + 2.0;
        Some(crate::world::entity_islands::HurtReach {
            area: self
                .get_entity()
                .bounding_box
                .load()
                .expand(reach, reach, reach),
            changes_blocks: false,
        })
    }

    fn pre_damage(&self, _damage_type: DamageType, source: Option<&dyn EntityBase>) -> bool {
        !(self.is_closed()
            && source.is_some_and(|source| is_abstract_arrow(source.get_entity().entity_type)))
    }
}

struct ShulkerAttackGoal {
    shulker: Arc<ShulkerEntity>,
    attack_cooldown: AtomicI32,
}

impl ShulkerAttackGoal {
    const fn new(shulker: Arc<ShulkerEntity>) -> Self {
        Self {
            shulker,
            attack_cooldown: AtomicI32::new(20),
        }
    }
}

impl Goal for ShulkerAttackGoal {
    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK
    }

    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let target = mob.get_mob_entity().get_target();
        target
            .as_ref()
            .is_some_and(|t| t.get_living_entity().is_some_and(|l| l.entity.is_alive()))
            && !is_peaceful(mob)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.can_start(mob)
    }

    fn start(&mut self, _mob: &dyn Mob) {
        self.attack_cooldown.store(20, Ordering::Relaxed);
        self.shulker.set_raw_peek(100);
    }

    fn stop(&mut self, _mob: &dyn Mob) {
        self.shulker.set_raw_peek(0);
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn tick(&mut self, mob: &dyn Mob) {
        if is_peaceful(mob) {
            return;
        }
        let cooldown = self.attack_cooldown.fetch_sub(1, Ordering::Relaxed) - 1;
        let mob_entity = mob.get_mob_entity();
        let Some(target) = mob_entity.get_target() else {
            return;
        };
        mob_entity
            .look_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .look_at_entity_with_range(&target, 180.0, 180.0);

        let entity = &mob_entity.living_entity.entity;
        let shulker_pos = entity.pos.load();
        let target_pos = target.get_entity().pos.load();
        if shulker_pos.squared_distance_to_vec(&target_pos) >= ATTACK_RANGE_SQ {
            mob_entity.set_target(None);
            return;
        }
        if cooldown > 0 {
            return;
        }
        self.attack_cooldown.store(
            20 + mob.get_random().random_range(0..10) * 20 / 2,
            Ordering::Relaxed,
        );
        let world = entity.world.load();
        let bullet = ShulkerBulletEntity::new(
            entity,
            target.get_entity(),
            self.shulker.get_attach_face().axis_of(),
        );
        world.spawn_entity_non_save(Arc::new(bullet));

        let pitch =
            1.0 + (mob.get_random().random::<f32>() - mob.get_random().random::<f32>()) * 0.2;
        world.play_sound_fine(
            Sound::EntityShulkerShoot,
            SoundCategory::Hostile,
            &shulker_pos,
            2.0,
            pitch,
        );
    }
}

struct ShulkerPeekGoal {
    shulker: Arc<ShulkerEntity>,
    peek_time: AtomicI32,
}

impl ShulkerPeekGoal {
    const fn new(shulker: Arc<ShulkerEntity>) -> Self {
        Self {
            shulker,
            peek_time: AtomicI32::new(0),
        }
    }
}

impl Goal for ShulkerPeekGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let has_target = mob.get_mob_entity().get_target().is_some();
        if has_target {
            return false;
        }
        if mob.get_random().random_range(0..to_goal_ticks(40)) != 0 {
            return false;
        }
        let pos = mob.get_mob_entity().living_entity.entity.block_pos.load();
        let face = self.shulker.get_attach_face();
        self.shulker.can_stay_at(&pos, face)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        let has_target = mob.get_mob_entity().get_target().is_some();
        !has_target && self.peek_time.load(Ordering::Relaxed) > 0
    }

    fn start(&mut self, mob: &dyn Mob) {
        let duration = self.get_tick_count(20 * (1 + mob.get_random().random_range(0..3)));
        self.peek_time.store(duration, Ordering::Relaxed);
        self.shulker.set_raw_peek(30);
    }

    fn stop(&mut self, _mob: &dyn Mob) {
        let has_target = self.shulker.mob_entity.get_target().is_some();
        if !has_target {
            self.shulker.set_raw_peek(0);
        }
    }

    fn tick(&mut self, _mob: &dyn Mob) {
        self.peek_time.fetch_sub(1, Ordering::Relaxed);
    }
}

trait BlockPosExt {
    fn offset_direction(&self, dir: BlockDirection) -> Self;
}

impl BlockPosExt for BlockPos {
    fn offset_direction(&self, dir: BlockDirection) -> Self {
        let offset = dir.to_offset();
        Self::new(
            self.0.x + offset.x,
            self.0.y + offset.y,
            self.0.z + offset.z,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opening_sweeps_only_the_space_the_lid_moves_into() {
        let at = Vector3::new(10.5, 64.0, -3.5);
        // A floor shulker opening from half to fully open sweeps the half block above its lid.
        let swept = progress_delta_aabb(1.0, BlockDirection::Up, 0.5, 1.0, at);
        assert_eq!(swept.min, Vector3::new(10.0, 65.5, -4.0));
        assert_eq!(swept.max, Vector3::new(11.0, 66.0, -3.0));
        // On a ceiling the shell opens downwards.
        let open = progress_aabb(2.0, BlockDirection::Down, 0.25, at);
        assert_eq!(open.min, Vector3::new(9.5, 63.5, -4.5));
        assert_eq!(open.max, Vector3::new(11.5, 66.0, -2.5));
    }
}
