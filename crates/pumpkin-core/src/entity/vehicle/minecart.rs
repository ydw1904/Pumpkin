mod chest;
mod container;
mod furnace;
mod hopper;
mod rideable;
mod tnt;

use pumpkin_util::random::gameplay::game_random;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use pumpkin_protocol::java::server::play::SPlayerInput;

use crate::{
    entity::{
        Entity, EntityBase, MINECART_PICKUP_GROWTH, MINECART_PUSH_GROWTH, living::LivingEntity,
        player::Player,
    },
    server::Server,
};
use pumpkin_data::Block;
use pumpkin_data::BlockStateId;
use pumpkin_data::block_properties::{
    PoweredRailLikeProperties, RailLikeProperties, RailShape, RailShapeStraight,
};
use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_inventory::Inventory;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::GameMode;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::math::wrap_degrees;

use crate::entity::vehicle::vehicle::VehicleEntity;
use crate::plugin::{
    api::events::vehicle::{vehicle_move::VehicleMoveEvent, vehicle_update::VehicleUpdateEvent},
    server::packet::PacketSentEvent,
};
use crate::world::World;
use crate::world::entity_islands::{self, AuditedReach};
use chest::ChestMinecart;
use container::MinecartInventory;
use furnace::FurnaceMinecart;
use hopper::HopperMinecart;
use rideable::RideableMinecart;
use tnt::TntMinecart;

const FLIPPED_ROTATION: &str = "FlippedRotation";
const GRAVITY: f64 = 0.04;
const WATER_GRAVITY: f64 = 0.005;
const AIR_DRAG: f64 = 0.95;
const RAIL_HEIGHT_OFFSET: f64 = 0.0625;
/// Vanilla `OldMinecartBehavior.MAX_SPEED_ON_LAND`, the fastest a minecart moves in a tick.
const MAX_SPEED_ON_LAND: f64 = 0.4;
const MAX_SPEED_IN_WATER: f64 = 0.2;
const SLOPE_SLIDE_SPEED: f64 = 0.007_812_5;
const POWERED_RAIL_BOOST: f64 = 0.06;
const POWERED_RAIL_START_SPEED: f64 = 0.02;

pub struct MinecartEntity {
    pub vehicle: VehicleEntity,
    kind: MinecartKind,
    /// Vanilla `onRails`.
    on_rails: AtomicBool,
    /// Vanilla `flipped`: the cart faces backwards along its track.
    flipped: AtomicBool,
}

enum MinecartKind {
    Rideable(RideableMinecart),
    Chest(ChestMinecart),
    Furnace(FurnaceMinecart),
    Hopper(HopperMinecart),
    Tnt(TntMinecart),
    Other,
}

impl MinecartEntity {
    pub fn new(mut entity: Entity) -> Self {
        entity.blocks_building = true;
        let kind = match entity.entity_type.id {
            id if id == EntityType::MINECART.id => MinecartKind::Rideable(RideableMinecart),
            id if id == EntityType::CHEST_MINECART.id => MinecartKind::Chest(ChestMinecart::new()),
            id if id == EntityType::FURNACE_MINECART.id => {
                MinecartKind::Furnace(FurnaceMinecart::new())
            }
            id if id == EntityType::HOPPER_MINECART.id => {
                MinecartKind::Hopper(HopperMinecart::new())
            }
            id if id == EntityType::TNT_MINECART.id => MinecartKind::Tnt(TntMinecart::new()),
            _ => MinecartKind::Other,
        };
        Self {
            vehicle: VehicleEntity::new(entity),
            kind,
            on_rails: AtomicBool::new(false),
            flipped: AtomicBool::new(false),
        }
    }

    const fn container(&self) -> Option<&Arc<MinecartInventory>> {
        match &self.kind {
            MinecartKind::Chest(minecart) => Some(minecart.inventory()),
            MinecartKind::Hopper(minecart) => Some(minecart.inventory()),
            _ => None,
        }
    }

    /// The inventory of a chest or hopper minecart.
    #[must_use]
    pub fn container_inventory(&self) -> Option<Arc<dyn pumpkin_inventory::Inventory>> {
        // Vanilla's container accessors all unpack the loot table first.
        self.container().map(|inventory| {
            inventory.unpack_loot(&self.vehicle.entity, None);
            inventory.clone() as Arc<dyn pumpkin_inventory::Inventory>
        })
    }

    /// Vanilla `getDropItem`. Command block and spawner minecarts drop a plain minecart.
    const fn drop_item(&self) -> &'static Item {
        match &self.kind {
            MinecartKind::Rideable(_) | MinecartKind::Other => &Item::MINECART,
            MinecartKind::Chest(_) => &Item::CHEST_MINECART,
            MinecartKind::Furnace(_) => &Item::FURNACE_MINECART,
            MinecartKind::Hopper(_) => &Item::HOPPER_MINECART,
            MinecartKind::Tnt(_) => &Item::TNT_MINECART,
        }
    }
}

/// Vanilla `AbstractMinecart.exits`: the two block offsets a rail shape connects.
const fn exits(shape: RailShape) -> ([i32; 3], [i32; 3]) {
    match shape {
        RailShape::NorthSouth => ([0, 0, -1], [0, 0, 1]),
        RailShape::EastWest => ([-1, 0, 0], [1, 0, 0]),
        RailShape::AscendingEast => ([-1, -1, 0], [1, 0, 0]),
        RailShape::AscendingWest => ([-1, 0, 0], [1, -1, 0]),
        RailShape::AscendingNorth => ([0, 0, -1], [0, -1, 1]),
        RailShape::AscendingSouth => ([0, -1, -1], [0, 0, 1]),
        RailShape::SouthEast => ([0, 0, 1], [1, 0, 0]),
        RailShape::SouthWest => ([0, 0, 1], [-1, 0, 0]),
        RailShape::NorthWest => ([0, 0, -1], [-1, 0, 0]),
        RailShape::NorthEast => ([0, 0, -1], [1, 0, 0]),
    }
}

/// Vanilla `BaseRailBlock.isRail`.
fn is_rail(block: &Block) -> bool {
    block.has_tag(&tag::Block::MINECRAFT_RAILS)
}

fn rail_shape(block: &Block, state_id: BlockStateId) -> RailShape {
    if block == &Block::RAIL {
        return RailLikeProperties::from_state_id(state_id).shape;
    }
    match PoweredRailLikeProperties::from_state_id(state_id).shape {
        RailShapeStraight::NorthSouth => RailShape::NorthSouth,
        RailShapeStraight::EastWest => RailShape::EastWest,
        RailShapeStraight::AscendingEast => RailShape::AscendingEast,
        RailShapeStraight::AscendingWest => RailShape::AscendingWest,
        RailShapeStraight::AscendingNorth => RailShape::AscendingNorth,
        RailShapeStraight::AscendingSouth => RailShape::AscendingSouth,
    }
}

const fn is_slope(shape: RailShape) -> bool {
    matches!(
        shape,
        RailShape::AscendingEast
            | RailShape::AscendingWest
            | RailShape::AscendingNorth
            | RailShape::AscendingSouth
    )
}

impl MinecartEntity {
    /// Vanilla `AbstractMinecart.getCurrentBlockPosOrRailBelow`.
    fn current_block_pos_or_rail_below(world: &World, pos: Vector3<f64>) -> BlockPos {
        let block_pos = BlockPos::floored(pos.x, pos.y, pos.z);
        if is_rail(world.get_block(&block_pos.down())) {
            block_pos.down()
        } else {
            block_pos
        }
    }

    /// Vanilla `Entity.applyGravity` with `AbstractMinecart.getDefaultGravity`.
    fn apply_gravity(&self) {
        let entity = &self.vehicle.entity;
        if entity.has_no_gravity() {
            return;
        }
        let gravity = if entity.is_in_water() {
            WATER_GRAVITY
        } else {
            GRAVITY
        };
        let velocity = entity.velocity.load();
        entity
            .velocity
            .store(Vector3::new(velocity.x, velocity.y - gravity, velocity.z));
    }

    /// Vanilla `AbstractMinecart.getMaxSpeed` with the furnace minecart's override.
    fn max_speed(&self) -> f64 {
        let in_water = self.vehicle.entity.is_in_water();
        let max_speed = if in_water {
            MAX_SPEED_IN_WATER
        } else {
            MAX_SPEED_ON_LAND
        };
        match (&self.kind, in_water) {
            (MinecartKind::Furnace(_), true) => max_speed * 0.75,
            (MinecartKind::Furnace(_), false) => max_speed * 0.5,
            _ => max_speed,
        }
    }

    /// Vanilla `AbstractMinecart.applyNaturalSlowdown` and its overrides.
    fn apply_natural_slowdown(&self, velocity: Vector3<f64>) -> Vector3<f64> {
        let entity = &self.vehicle.entity;
        if let MinecartKind::Furnace(minecart) = &self.kind {
            return minecart.velocity(entity, velocity);
        }
        if let Some(inventory) = self.container() {
            return container::velocity(entity, inventory, velocity);
        }
        let slowdown = if entity.has_passengers() { 0.997 } else { 0.96 };
        let slowed = velocity.multiply(slowdown, 0.0, slowdown);
        if entity.is_in_water() {
            slowed.multiply(0.95, 0.95, 0.95)
        } else {
            slowed
        }
    }

    /// Vanilla `AbstractMinecart.move(MoverType.SELF, ...)`.
    fn move_self(&self, caller: &dyn EntityBase, delta: Vector3<f64>) {
        let entity = &self.vehicle.entity;
        let from = entity.pos.load();
        let to_position = from.add(&delta);
        entity.move_entity(caller, delta);
        if let Some(dyn_self) = entity.world.load().get_entity_by_id(entity.entity_id)
            && dyn_self.push_entities(caller)
        {
            let back_motion = to_position.sub(&entity.pos.load());
            entity.move_entity(caller, back_motion);
        }
        let to = entity.pos.load();
        let allowed = super::vehicle::fire_vehicle_move(entity, from, to);
        if allowed != to {
            entity.set_pos(allowed);
        }
    }

    /// Vanilla `AbstractMinecart.comeOffTrack`.
    fn come_off_track(&self, caller: &dyn EntityBase) {
        let entity = &self.vehicle.entity;
        let max_speed = self.max_speed();
        let velocity = entity.velocity.load();
        let mut velocity = Vector3::new(
            velocity.x.clamp(-max_speed, max_speed),
            velocity.y,
            velocity.z.clamp(-max_speed, max_speed),
        );
        if entity.on_ground.load(Ordering::Relaxed) {
            velocity = velocity.multiply(0.5, 0.5, 0.5);
        }
        entity.velocity.store(velocity);
        self.move_self(caller, velocity);
        if !entity.on_ground.load(Ordering::Relaxed) {
            let velocity = entity.velocity.load();
            entity
                .velocity
                .store(velocity.multiply(AIR_DRAG, AIR_DRAG, AIR_DRAG));
        }
    }

    /// Vanilla `OldMinecartBehavior.moveAlongTrack`.
    #[expect(clippy::too_many_lines)]
    fn move_along_track(
        &self,
        caller: &dyn EntityBase,
        world: &World,
        pos: BlockPos,
        block: &Block,
        state_id: BlockStateId,
    ) {
        let entity = &self.vehicle.entity;
        let start = entity.pos.load();
        let old_pos = Self::rail_pos(world, start);
        let mut y = f64::from(pos.0.y);
        let (power_track, mut halt_track) = if block == &Block::POWERED_RAIL {
            let powered = PoweredRailLikeProperties::from_state_id(state_id).powered;
            (powered, !powered)
        } else {
            (false, false)
        };

        let slide_speed = if entity.is_in_water() {
            SLOPE_SLIDE_SPEED * 0.2
        } else {
            SLOPE_SLIDE_SPEED
        };
        let shape = rail_shape(block, state_id);
        let mut velocity = entity.velocity.load();
        match shape {
            RailShape::AscendingEast => velocity.x -= slide_speed,
            RailShape::AscendingWest => velocity.x += slide_speed,
            RailShape::AscendingNorth => velocity.z += slide_speed,
            RailShape::AscendingSouth => velocity.z -= slide_speed,
            _ => {}
        }
        if is_slope(shape) {
            y += 1.0;
        }

        let (exit0, exit1) = exits(shape);
        let mut x_d = f64::from(exit1[0] - exit0[0]);
        let mut z_d = f64::from(exit1[2] - exit0[2]);
        let length = x_d.hypot(z_d);
        if velocity.x * x_d + velocity.z * z_d < 0.0 {
            x_d = -x_d;
            z_d = -z_d;
        }
        let pow = velocity.horizontal_length().min(2.0);
        velocity = Vector3::new(pow * x_d / length, velocity.y, pow * z_d / length);

        // A rider can only nudge a cart that has nearly stopped.
        let intent = entity
            .passengers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .first()
            .and_then(|passenger| passenger.get_player().map(Self::last_client_move_intent));
        if let Some(intent) = intent
            && intent.length_squared() > 0.0
            && velocity.horizontal_length_squared() < 0.01
        {
            velocity.x += intent.x * 0.001;
            velocity.z += intent.z * 0.001;
            halt_track = false;
        }

        if halt_track {
            if velocity.horizontal_length() < 0.03 {
                velocity = Vector3::default();
            } else {
                velocity = velocity.multiply(0.5, 0.0, 0.5);
            }
        }

        let x0 = f64::from(pos.0.x) + 0.5 + f64::from(exit0[0]) * 0.5;
        let z0 = f64::from(pos.0.z) + 0.5 + f64::from(exit0[2]) * 0.5;
        let x1 = f64::from(pos.0.x) + 0.5 + f64::from(exit1[0]) * 0.5;
        let z1 = f64::from(pos.0.z) + 0.5 + f64::from(exit1[2]) * 0.5;
        let x_d = x1 - x0;
        let z_d = z1 - z0;
        let progress = if x_d == 0.0 {
            start.z - f64::from(pos.0.z)
        } else if z_d == 0.0 {
            start.x - f64::from(pos.0.x)
        } else {
            ((start.x - x0) * x_d + (start.z - z0) * z_d) * 2.0
        };
        entity.set_pos(Vector3::new(x0 + x_d * progress, y, z0 + z_d * progress));
        entity.velocity.store(velocity);

        let scale = if entity.has_passengers() { 0.75 } else { 1.0 };
        let max_speed = self.max_speed();
        self.move_self(
            caller,
            Vector3::new(
                (scale * velocity.x).clamp(-max_speed, max_speed),
                0.0,
                (scale * velocity.z).clamp(-max_speed, max_speed),
            ),
        );
        let moved = entity.pos.load();
        let (block_x, block_z) = (moved.x.floor() as i32, moved.z.floor() as i32);
        for exit in [exit0, exit1] {
            if exit[1] != 0 && block_x - pos.0.x == exit[0] && block_z - pos.0.z == exit[2] {
                entity.set_pos(Vector3::new(moved.x, moved.y + f64::from(exit[1]), moved.z));
                break;
            }
        }

        let mut velocity = self.apply_natural_slowdown(entity.velocity.load());
        let current = entity.pos.load();
        if let (Some(new_pos), Some(old_pos)) = (Self::rail_pos(world, current), old_pos) {
            let speed = (old_pos.y - new_pos.y) * 0.05;
            let horizontal = velocity.horizontal_length();
            if horizontal > 0.0 {
                let factor = (horizontal + speed) / horizontal;
                velocity = velocity.multiply(factor, 1.0, factor);
            }
            entity.set_pos(Vector3::new(current.x, new_pos.y, current.z));
        }

        let current = entity.pos.load();
        let (block_x, block_z) = (current.x.floor() as i32, current.z.floor() as i32);
        if block_x != pos.0.x || block_z != pos.0.z {
            let horizontal = velocity.horizontal_length();
            velocity = Vector3::new(
                horizontal * f64::from(block_x - pos.0.x),
                velocity.y,
                horizontal * f64::from(block_z - pos.0.z),
            );
        }

        if power_track {
            let horizontal = velocity.horizontal_length();
            if horizontal > 0.01 {
                velocity.x += velocity.x / horizontal * POWERED_RAIL_BOOST;
                velocity.z += velocity.z / horizontal * POWERED_RAIL_BOOST;
            } else {
                // A cart at rest is pushed away from a solid block at either end.
                let conductor = |pos: BlockPos| world.get_block_state(&pos).is_solid_block();
                match shape {
                    RailShape::EastWest => {
                        if conductor(pos.west()) {
                            velocity.x = POWERED_RAIL_START_SPEED;
                        } else if conductor(pos.east()) {
                            velocity.x = -POWERED_RAIL_START_SPEED;
                        }
                    }
                    RailShape::NorthSouth => {
                        if conductor(pos.north()) {
                            velocity.z = POWERED_RAIL_START_SPEED;
                        } else if conductor(pos.south()) {
                            velocity.z = -POWERED_RAIL_START_SPEED;
                        }
                    }
                    _ => {}
                }
            }
        }
        entity.velocity.store(velocity);
    }

    /// Vanilla `ServerPlayer.getLastClientMoveIntent`.
    fn last_client_move_intent(player: &Player) -> Vector3<f64> {
        let input = player.last_input.load(Ordering::Relaxed);
        let impulse =
            |positive: i8, negative: i8| match (input & positive != 0, input & negative != 0) {
                (true, false) => 1.0,
                (false, true) => -1.0,
                _ => 0.0,
            };
        player.get_entity().movement_input_to_velocity(
            Vector3::new(
                impulse(SPlayerInput::LEFT, SPlayerInput::RIGHT),
                0.0,
                impulse(SPlayerInput::FORWARD, SPlayerInput::BACKWARD),
            ),
            1.0,
        )
    }

    /// Vanilla `OldMinecartBehavior.getPos`: the point on the rail line at or below `pos`.
    fn rail_pos(world: &World, pos: Vector3<f64>) -> Option<Vector3<f64>> {
        let rail_pos = Self::current_block_pos_or_rail_below(world, pos);
        let (block, state_id) = world.get_block_and_state_id(&rail_pos);
        if !is_rail(block) {
            return None;
        }
        let (exit0, exit1) = exits(rail_shape(block, state_id));
        let (xt, yt, zt) = (
            f64::from(rail_pos.0.x),
            f64::from(rail_pos.0.y),
            f64::from(rail_pos.0.z),
        );
        let x0 = xt + 0.5 + f64::from(exit0[0]) * 0.5;
        let y0 = yt + RAIL_HEIGHT_OFFSET + f64::from(exit0[1]) * 0.5;
        let z0 = zt + 0.5 + f64::from(exit0[2]) * 0.5;
        let x1 = xt + 0.5 + f64::from(exit1[0]) * 0.5;
        let y1 = yt + RAIL_HEIGHT_OFFSET + f64::from(exit1[1]) * 0.5;
        let z1 = zt + 0.5 + f64::from(exit1[2]) * 0.5;
        let x_d = x1 - x0;
        let y_d = (y1 - y0) * 2.0;
        let z_d = z1 - z0;
        let progress = if x_d == 0.0 {
            pos.z - zt
        } else if z_d == 0.0 {
            pos.x - xt
        } else {
            ((pos.x - x0) * x_d + (pos.z - z0) * z_d) * 2.0
        };
        let mut y = y0 + y_d * progress;
        if y_d < 0.0 {
            y += 1.0;
        } else if y_d > 0.0 {
            y += 0.5;
        }
        Some(Vector3::new(x0 + x_d * progress, y, z0 + z_d * progress))
    }

    /// Vanilla `activateMinecart`, run for a cart on an activator rail.
    fn activate_minecart(&self, powered: bool) {
        match &self.kind {
            MinecartKind::Hopper(minecart) => minecart.set_enabled(!powered),
            MinecartKind::Tnt(minecart) if powered => minecart.prime(&self.vehicle.entity, 80),
            MinecartKind::Rideable(_) if powered => {
                self.vehicle.entity.eject_passengers();
                if self.vehicle.get_hurt_time() == 0 {
                    self.vehicle.set_hurt_dir(-self.vehicle.get_hurt_dir());
                    self.vehicle.set_hurt_time(10);
                    self.vehicle.set_damage(50.0);
                    self.vehicle.send_wobble_metadata();
                }
            }
            _ => {}
        }
    }

    /// The rotation part of vanilla `OldMinecartBehavior.tick`: the cart turns the way it
    /// moved, and flips instead of turning around.
    fn update_rotation(&self, start_pos: Vector3<f64>, start_yaw: f32) {
        let entity = &self.vehicle.entity;
        let pos = entity.pos.load();
        let mut flipped = self.flipped.load(Ordering::Relaxed);
        let mut yaw = entity.yaw.load();
        let x_diff = start_pos.x - pos.x;
        let z_diff = start_pos.z - pos.z;
        if x_diff * x_diff + z_diff * z_diff > 0.001 {
            yaw = z_diff.atan2(x_diff).to_degrees() as f32;
            if flipped {
                yaw += 180.0;
            }
        }
        let rot_diff = wrap_degrees(yaw - start_yaw);
        if !(-170.0..170.0).contains(&rot_diff) {
            yaw += 180.0;
            flipped = !flipped;
            self.flipped.store(flipped, Ordering::Relaxed);
        }
        entity.set_rotation(yaw % 360.0, 0.0);
    }
}

const LOWERED_PASSENGER_ATTACHMENT: Vector3<f64> = Vector3::new(0.0, 0.0, 0.0);

impl MinecartEntity {
    /// What an empty rideable or chest minecart can touch next tick.
    fn empty_cart_reach(&self) -> Option<AuditedReach> {
        let entity = &self.vehicle.entity;
        let rideable = matches!(self.kind, MinecartKind::Rideable(_));
        if !(rideable || matches!(self.kind, MinecartKind::Chest(_)))
            || entity.has_passengers()
            || entity.has_vehicle()
        {
            return None;
        }
        let world = entity.world.load();
        // Plugins see every tick through these events.
        if world.server.upgrade().is_some_and(|server| {
            server.plugin_manager.has_handlers::<VehicleUpdateEvent>()
                || server.plugin_manager.has_handlers::<VehicleMoveEvent>()
                || server.plugin_manager.has_handlers::<PacketSentEvent>()
        }) {
            return None;
        }
        let fluid_push = super::super::max_fluid_push_speed(world.dimension.fast_lava);
        // A powered rail can speed it up to its maximum, it falls off the rails, and a rail
        // first moves it onto the rail's line in its block or the block below.
        let speed = entity.velocity.load().length().max(MAX_SPEED_ON_LAND) + GRAVITY + fluid_push;
        let motion = (speed + std::f64::consts::SQRT_2, speed + 2.0);
        let mut links = entity_islands::links_of(self);
        let moved = entity
            .bounding_box
            .load()
            .expand(motion.0, motion.1, motion.0);
        super::super::mob::game_event_listener_links(&world, true, &moved, &mut links);
        let mut reach = AuditedReach {
            bounding_box: entity.bounding_box.load(),
            motion,
            // A rideable minecart takes passengers after moving, and both kinds push what they
            // touch then.
            entities: if rideable {
                MINECART_PICKUP_GROWTH
            } else {
                MINECART_PUSH_GROWTH
            },
            players: MINECART_PUSH_GROWTH,
            typed_entities: Vec::new(),
            kind: Some(entity.entity_type),
            // The rail in its block or the block below.
            block_reads: 2.0,
            route: None,
            changes_blocks: false,
            global: false,
            links,
            sight: Vec::new(),
            spans_contacts: false,
            // Pushing another minecart shares their speeds, more than an ordinary push.
            knockback: MINECART_PUSH_GROWTH,
        };
        reach.scan_contact_blocks(&world);
        Some(reach)
    }
}

impl EntityBase for MinecartEntity {
    fn audited_reach(&self) -> Option<AuditedReach> {
        self.empty_cart_reach()
    }

    fn passenger_attachment_point(
        &self,
        passenger: &dyn EntityBase,
        _index: usize,
    ) -> Option<Vector3<f64>> {
        let passenger_type = passenger.get_entity().entity_type;
        (passenger_type == &EntityType::VILLAGER || passenger_type == &EntityType::WANDERING_TRADER)
            .then_some(LOWERED_PASSENGER_ATTACHMENT)
    }

    fn write_custom_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_bool(FLIPPED_ROTATION, self.flipped.load(Ordering::Relaxed));
        match &self.kind {
            MinecartKind::Chest(minecart) => minecart.write_nbt(nbt),
            MinecartKind::Furnace(minecart) => minecart.write_nbt(nbt),
            MinecartKind::Hopper(minecart) => minecart.write_nbt(nbt),
            MinecartKind::Tnt(minecart) => minecart.write_nbt(nbt),
            MinecartKind::Rideable(_) | MinecartKind::Other => {}
        }
    }

    fn read_custom_nbt(&self, nbt: &NbtCompound) {
        self.flipped.store(
            nbt.get_bool(FLIPPED_ROTATION).unwrap_or(false),
            Ordering::Relaxed,
        );
        match &self.kind {
            MinecartKind::Chest(minecart) => minecart.read_nbt(nbt),
            MinecartKind::Furnace(minecart) => minecart.read_nbt(nbt),
            MinecartKind::Hopper(minecart) => minecart.read_nbt(nbt),
            MinecartKind::Tnt(minecart) => minecart.read_nbt(nbt),
            MinecartKind::Rideable(_) | MinecartKind::Other => {}
        }
    }

    #[allow(clippy::too_many_lines)]
    fn tick(&self, caller: &dyn EntityBase, _server: &Server) {
        self.vehicle.tick();
        self.vehicle.entity.check_out_of_world(caller);
        if self.vehicle.entity.is_removed() {
            return;
        }
        // Vanilla `handlePortal` is left out: non-player entities can't change dimension yet,
        // so the minecart would stay behind while its rider is teleported.
        if let MinecartKind::Furnace(minecart) = &self.kind {
            minecart.tick(&self.vehicle.entity);
        }

        let entity = &self.vehicle.entity;
        let world = entity.world.load();
        let start_pos = entity.pos.load();
        let start_yaw = entity.yaw.load();

        // Vanilla `OldMinecartBehavior.tick`.
        self.apply_gravity();
        let rail_pos = Self::current_block_pos_or_rail_below(&world, entity.pos.load());
        let (block, state_id) = world.get_block_and_state_id(&rail_pos);
        let on_rails = is_rail(block);
        self.on_rails.store(on_rails, Ordering::Relaxed);
        if on_rails {
            self.move_along_track(caller, &world, rail_pos, block, state_id);
            if block == &Block::ACTIVATOR_RAIL {
                self.activate_minecart(PoweredRailLikeProperties::from_state_id(state_id).powered);
            }
        } else {
            self.come_off_track(caller);
        }

        // Vanilla `OldMinecartBehavior.tick`: `applyEffectsFromBlocks` after moving.
        self.vehicle.entity.tick_block_collisions(caller);
        self.update_rotation(start_pos, start_yaw);

        // Vanilla `AbstractMinecart.tick` hurts minecarts in lava on top of the lava fluid effect.
        self.vehicle.entity.update_fluid_state(caller);
        if self.vehicle.entity.touching_lava.load(Ordering::SeqCst) {
            self.vehicle.entity.lava_ignite(caller);
            self.vehicle.entity.lava_hurt(caller);
        }
        self.vehicle
            .entity
            .first_tick
            .store(false, Ordering::Relaxed);

        match &self.kind {
            MinecartKind::Hopper(minecart) => minecart.tick(&self.vehicle.entity),
            // Vanilla `MinecartTNT.tick` after the minecart's own tick.
            MinecartKind::Tnt(minecart) => {
                if minecart.tick(&self.vehicle.entity) {
                    return;
                }
                let velocity = self.vehicle.entity.velocity.load();
                let speed_squared = velocity.horizontal_length_squared();
                if self
                    .vehicle
                    .entity
                    .horizontal_collision
                    .load(Ordering::Relaxed)
                    && speed_squared >= 0.01
                {
                    minecart.explode(&self.vehicle.entity, speed_squared);
                }
            }
            _ => {}
        }
    }

    fn get_entity(&self) -> &Entity {
        &self.vehicle.entity
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        None
    }

    fn is_pushable(&self) -> bool {
        true
    }

    fn is_on_rails(&self) -> bool {
        self.on_rails.load(Ordering::Relaxed)
    }

    #[allow(clippy::too_many_lines)]
    fn push(&self, entity: &dyn EntityBase) {
        let self_entity = self.get_entity();
        let other_entity = entity.get_entity();

        if self_entity.no_physics.load(Ordering::Relaxed)
            || other_entity.no_physics.load(Ordering::Relaxed)
        {
            return;
        }

        {
            let passengers = self_entity
                .passengers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if passengers
                .iter()
                .any(|p| p.get_entity().entity_id == other_entity.entity_id)
            {
                return;
            }
        }
        {
            let passengers = other_entity
                .passengers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if passengers
                .iter()
                .any(|p| p.get_entity().entity_id == self_entity.entity_id)
            {
                return;
            }
        }

        let mut xa = other_entity.pos.load().x - self_entity.pos.load().x;
        let mut za = other_entity.pos.load().z - self_entity.pos.load().z;
        let mut dd = xa * xa + za * za;
        if dd >= 1.0E-4 {
            dd = dd.sqrt();
            xa /= dd;
            za /= dd;
            let mut pow = 1.0 / dd;
            if pow > 1.0 {
                pow = 1.0;
            }
            xa *= pow;
            za *= pow;
            xa *= 0.1;
            za *= 0.1;
            xa *= 0.5;
            za *= 0.5;

            let is_other_minecart = other_entity.entity_type.id == EntityType::MINECART.id
                || other_entity.entity_type.id == EntityType::CHEST_MINECART.id
                || other_entity.entity_type.id == EntityType::COMMAND_BLOCK_MINECART.id
                || other_entity.entity_type.id == EntityType::FURNACE_MINECART.id
                || other_entity.entity_type.id == EntityType::HOPPER_MINECART.id
                || other_entity.entity_type.id == EntityType::SPAWNER_MINECART.id
                || other_entity.entity_type.id == EntityType::TNT_MINECART.id;

            if is_other_minecart {
                let xo = self_entity.velocity.load().x;
                let zo = self_entity.velocity.load().z;

                let dir = Vector3::new(xo, 0.0, zo).normalize();
                let facing = Vector3::new(
                    f64::from(self_entity.yaw.load().to_radians().cos()),
                    0.0,
                    f64::from(self_entity.yaw.load().to_radians().sin()),
                )
                .normalize();

                let dot = dir.dot(&facing).abs();
                if dot >= 0.8 {
                    let vel = self_entity.velocity.load();
                    let ovel = other_entity.velocity.load();

                    let is_self_furnace =
                        self_entity.entity_type.id == EntityType::FURNACE_MINECART.id;
                    let is_other_furnace =
                        other_entity.entity_type.id == EntityType::FURNACE_MINECART.id;

                    if is_other_furnace && !is_self_furnace {
                        self_entity.velocity.store(vel.multiply(0.2, 1.0, 0.2));
                        let mut new_self_vel = self_entity.velocity.load();
                        new_self_vel.x += ovel.x - xa;
                        new_self_vel.z += ovel.z - za;
                        self_entity.velocity.store(new_self_vel);
                        self_entity.send_velocity();

                        other_entity.velocity.store(ovel.multiply(0.95, 1.0, 0.95));
                        other_entity.send_velocity();
                    } else if !is_other_furnace && is_self_furnace {
                        other_entity.velocity.store(ovel.multiply(0.2, 1.0, 0.2));
                        let mut new_other_vel = other_entity.velocity.load();
                        new_other_vel.x += vel.x + xa;
                        new_other_vel.z += vel.z + za;
                        other_entity.velocity.store(new_other_vel);
                        other_entity.send_velocity();

                        self_entity.velocity.store(vel.multiply(0.95, 1.0, 0.95));
                        self_entity.send_velocity();
                    } else {
                        #[allow(clippy::manual_midpoint)]
                        let xdd = (ovel.x + vel.x) / 2.0;
                        #[allow(clippy::manual_midpoint)]
                        let zdd = (ovel.z + vel.z) / 2.0;

                        self_entity.velocity.store(vel.multiply(0.2, 1.0, 0.2));
                        let mut new_self_vel = self_entity.velocity.load();
                        new_self_vel.x += xdd - xa;
                        new_self_vel.z += zdd - za;
                        self_entity.velocity.store(new_self_vel);
                        self_entity.send_velocity();

                        other_entity.velocity.store(ovel.multiply(0.2, 1.0, 0.2));
                        let mut new_other_vel = other_entity.velocity.load();
                        new_other_vel.x += xdd + xa;
                        new_other_vel.z += zdd + za;
                        other_entity.velocity.store(new_other_vel);
                        other_entity.send_velocity();
                    }
                }
            } else {
                if !self_entity.has_passengers() && self.is_pushable() {
                    let mut vel = self_entity.velocity.load();
                    vel.x -= xa;
                    vel.z -= za;
                    self_entity.velocity.store(vel);
                    self_entity.send_velocity();
                }

                if !other_entity.has_passengers() && entity.is_pushable() {
                    let mut vel = other_entity.velocity.load();
                    vel.x += xa / 4.0;
                    vel.z += za / 4.0;
                    other_entity.velocity.store(vel);
                    other_entity.send_velocity();
                }
            }
        }
    }

    fn is_collidable(&self, _entity: Option<Box<dyn EntityBase>>) -> bool {
        true
    }

    fn init_data_tracker(&self) {
        self.vehicle.send_wobble_metadata();
        if let MinecartKind::Furnace(minecart) = &self.kind {
            minecart.init_data_tracker(&self.vehicle.entity);
        }
    }

    fn can_hit(&self) -> bool {
        self.vehicle.entity.is_alive()
    }

    fn damage_with_context(
        &self,
        _caller: &dyn EntityBase,
        amount: f32,
        damage_type: DamageType,
        _position: Option<Vector3<f64>>,
        source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> bool {
        let creative = source
            .and_then(EntityBase::get_player)
            .is_some_and(|player| player.gamemode.load() == GameMode::Creative);

        if let MinecartKind::Tnt(minecart) = &self.kind
            && damage_type == DamageType::ARROW
            && self.vehicle.entity.fire_ticks.load(Ordering::Relaxed) > 0
        {
            let projectile_speed_squared = cause
                .map(|entity| entity.get_entity().velocity.load().length_squared())
                .unwrap_or_default();
            minecart.explode(&self.vehicle.entity, projectile_speed_squared);
            if self.vehicle.entity.is_removed() {
                return true;
            }
        }

        let will_break = self.vehicle.entity.is_alive()
            && (creative || self.vehicle.get_damage() + amount * 10.0 > 40.0);

        if let MinecartKind::Tnt(minecart) = &self.kind
            && will_break
            && !creative
        {
            let velocity = self.vehicle.entity.velocity.load();
            let speed_squared = velocity.x.mul_add(velocity.x, velocity.z * velocity.z);
            let ignites = damage_type.has_tag(&tag::DamageType::MINECRAFT_IS_FIRE)
                || damage_type.has_tag(&tag::DamageType::MINECRAFT_IS_EXPLOSION)
                || self.vehicle.entity.fire_ticks.load(Ordering::Relaxed) > 0;
            if ignites || speed_squared >= 0.01 {
                self.vehicle.apply_damage_wobble(amount);
                let fuse = game_random().random_range(0..20) + game_random().random_range(0..20);
                if self
                    .vehicle
                    .entity
                    .world
                    .load()
                    .level_info
                    .load()
                    .game_rules
                    .tnt_explodes
                {
                    minecart.prime(&self.vehicle.entity, fuse);
                } else {
                    minecart.set_fuse(fuse);
                }
                return true;
            }
        }

        let damaged = self.vehicle.damage_with_context(amount, source);

        if will_break && !creative && self.vehicle.entity.is_removed() {
            let world = self.vehicle.entity.world.load();
            if world.level_info.load().game_rules.entity_drops {
                let position = self.vehicle.entity.block_pos.load();
                if let Some(container) = self.container()
                    && container.claim_drops()
                {
                    container.unpack_loot(&self.vehicle.entity, None);
                    let inventory: Arc<dyn Inventory> = container.clone();
                    world.scatter_inventory(&position, &inventory);
                }
                world.drop_stack(&position, ItemStack::new(1, self.drop_item()));
            }
        }

        damaged
    }

    fn interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        match &self.kind {
            MinecartKind::Chest(minecart) => {
                minecart.interact(&self.vehicle.entity, player);
                true
            }
            MinecartKind::Furnace(minecart) => {
                minecart.interact(&self.vehicle.entity, player, item_stack)
            }
            MinecartKind::Hopper(minecart) => {
                minecart.interact(&self.vehicle.entity, player);
                true
            }
            MinecartKind::Rideable(_) => RideableMinecart::interact(&self.vehicle.entity, player),
            MinecartKind::Tnt(_) | MinecartKind::Other => false,
        }
    }

    fn on_player_collision(&self, player: &Arc<Player>) {
        if self.vehicle.entity.has_passenger(player.entity_id()) {
            return;
        }

        if player.is_spectator() {
            return;
        }

        let player_pos = player.get_entity().pos.load();
        let minecart_pos = self.vehicle.entity.pos.load();

        let mut diff_x = minecart_pos.x - player_pos.x;
        let mut diff_z = minecart_pos.z - player_pos.z;

        let dist_sq = diff_x * diff_x + diff_z * diff_z;
        if dist_sq > 0.0001 {
            let dist = dist_sq.sqrt();
            diff_x /= dist;
            diff_z /= dist;

            let push_force = 0.1;
            let mut vel = self.vehicle.entity.velocity.load();
            vel.x += diff_x * push_force;
            vel.z += diff_z * push_force;

            let horizontal_speed = vel.x.hypot(vel.z);
            if horizontal_speed > 0.4 {
                vel.x = (vel.x / horizontal_speed) * 0.4;
                vel.z = (vel.z / horizontal_speed) * 0.4;
            }

            self.vehicle.entity.velocity.store(vel);
            self.vehicle.entity.send_velocity();
        }
    }

    fn move_entity(&self, caller: &dyn EntityBase, motion: Vector3<f64>) {
        self.move_self(caller, motion);
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }
}
