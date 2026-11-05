mod chest;
mod container;
mod physics;

use std::f32::consts::{FRAC_PI_4, FRAC_PI_8, TAU};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::entity::{EntityStatus, EntityType};
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::translation;
use pumpkin_inventory::Inventory;
use pumpkin_nbt::NbtCompound;
use pumpkin_protocol::bedrock::server::actor_event::ActorEventID;
use pumpkin_util::GameMode;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::wrap_degrees;
use pumpkin_util::random::gameplay::game_random;
use pumpkin_util::text::TextComponent;

use crate::entity::living::is_allowed_by_team_rules;
use crate::entity::player::Player;
use crate::entity::vehicle::boat::chest::ChestBoat;
use crate::entity::{Entity, EntityBase, living::LivingEntity, push_apart};
use crate::server::Server;
use crate::world::game_event::GameEventContext;

use pumpkin_data::damage::DamageType;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_protocol::java::client::play::Metadata;

use pumpkin_util::math::vector3::Vector3;

use crate::entity::vehicle::vehicle::VehicleEntity;
use crate::plugin::{
    api::events::{
        entity::{entity_dismount::EntityDismountEvent, entity_mount::EntityMountEvent},
        vehicle::{
            vehicle_damage::VehicleDamageEvent, vehicle_destroy::VehicleDestroyEvent,
            vehicle_enter::VehicleEnterEvent, vehicle_exit::VehicleExitEvent,
            vehicle_update::VehicleUpdateEvent,
        },
    },
    server::packet::PacketSentEvent,
};
use crate::world::entity_islands::{self, AuditedReach};
use physics::Status;

const FRONT_SEAT_OFFSET: f32 = 0.2;
const BACK_SEAT_OFFSET: f32 = -0.6;
const ANIMAL_SEAT_SHIFT: f32 = 0.2;

/// Vanilla `AbstractBoat.TIME_TO_EJECT`: ticks under water before the passengers fall out.
const TIME_TO_EJECT: f32 = 60.0;
/// Vanilla `AbstractBoat.BUBBLE_TIME`.
const BUBBLE_TIME: i32 = 60;
/// Vanilla `AbstractBoat.PADDLE_SPEED`.
const PADDLE_SPEED: f32 = FRAC_PI_8;
/// Vanilla `AbstractBoat.PADDLE_SOUND_TIME`.
const PADDLE_SOUND_TIME: f32 = FRAC_PI_4;
/// What a downward bubble column adds to the boat's vertical speed when it sinks it.
const BUBBLE_SINK_SPEED: f64 = -0.7;
/// The vertical speed an upward bubble column launches the boat with.
const BUBBLE_LAUNCH_SPEED: f64 = 0.6;
/// One in this many ticks above a bubble column splashes.
const BUBBLE_SPLASH_CHANCE: u32 = 100;
/// The tick's box `inflate(0.2F, -0.01F, 0.2F)` that finds what it pushes or picks up.
const PUSH_GROWTH: f64 = 0.2f32 as f64;
const PUSH_HEIGHT_GROWTH: f64 = -0.01f32 as f64;
const MAX_PASSENGERS: usize = 2;
const CHEST_BOAT_MAX_PASSENGERS: usize = 1;
/// The most `floatBoat` buoyancy and the lift onto the surface of water a falling boat lands in
/// move it vertically, on top of its speed and last fall.
const MAX_FLOAT_MOVEMENT: f64 = 3.0;
/// Blocks read beyond the moved box: the ground under it and the dismount spots around it.
const BLOCK_READS: f64 = 2.0;

pub struct BoatEntity {
    pub vehicle: VehicleEntity,
    kind: BoatKind,
    status: AtomicCell<Option<Status>>,
    out_of_control_ticks: AtomicCell<f32>,
    water_level: AtomicCell<f64>,
    land_friction: AtomicCell<f32>,
    /// The vertical speed of its last move.
    last_yd: AtomicCell<f64>,
    is_above_bubble_column: AtomicBool,
    bubble_column_direction_is_down: AtomicBool,
    bubble_time: AtomicI32,
    paddle_positions: [AtomicCell<f32>; 2],
    left_paddle_moving: AtomicBool,
    right_paddle_moving: AtomicBool,
}

pub enum BoatKind {
    Normal,
    Chest(ChestBoat),
}

impl BoatEntity {
    pub fn new(mut entity: Entity) -> Self {
        entity.blocks_building = true;
        let kind = match entity.entity_type.id {
            id if id == EntityType::ACACIA_CHEST_BOAT.id
                || id == EntityType::BIRCH_CHEST_BOAT.id
                || id == EntityType::DARK_OAK_CHEST_BOAT.id
                || id == EntityType::JUNGLE_CHEST_BOAT.id
                || id == EntityType::MANGROVE_CHEST_BOAT.id
                || id == EntityType::OAK_CHEST_BOAT.id
                || id == EntityType::PALE_OAK_CHEST_BOAT.id
                || id == EntityType::SPRUCE_CHEST_BOAT.id
                || id == EntityType::BAMBOO_CHEST_RAFT.id
                || id == EntityType::CHERRY_CHEST_BOAT.id
                || id == EntityType::POPLAR_CHEST_BOAT.id =>
            {
                BoatKind::Chest(ChestBoat::new())
            }
            _ => BoatKind::Normal,
        };
        Self {
            vehicle: VehicleEntity::new(entity),
            kind,
            status: AtomicCell::new(None),
            out_of_control_ticks: AtomicCell::new(0.0),
            water_level: AtomicCell::new(0.0),
            land_friction: AtomicCell::new(0.0),
            last_yd: AtomicCell::new(0.0),
            is_above_bubble_column: AtomicBool::new(false),
            bubble_column_direction_is_down: AtomicBool::new(false),
            bubble_time: AtomicI32::new(0),
            paddle_positions: [AtomicCell::new(0.0), AtomicCell::new(0.0)],
            left_paddle_moving: AtomicBool::new(false),
            right_paddle_moving: AtomicBool::new(false),
        }
    }

    /// Vanilla `setPaddleState`. Like synched data, only a change is sent.
    pub fn set_paddles(&self, left: bool, right: bool) {
        let old_left = self.left_paddle_moving.swap(left, Ordering::Relaxed);
        let old_right = self.right_paddle_moving.swap(right, Ordering::Relaxed);
        if old_left == left && old_right == right {
            return;
        }

        self.vehicle.entity.send_meta_data(
            &[
                Metadata::new(pumpkin_data::tracked_data::boat::ID_PADDLE_LEFT, left),
                Metadata::new(pumpkin_data::tracked_data::boat::ID_PADDLE_RIGHT, right),
            ],
            None,
        );
    }

    /// Vanilla `getPaddleState`.
    fn get_paddle_state(&self, side: usize) -> bool {
        let moving = if side == 0 {
            &self.left_paddle_moving
        } else {
            &self.right_paddle_moving
        };
        moving.load(Ordering::Relaxed) && self.get_controlling_passenger().is_some()
    }

    fn set_bubble_time(&self, time: i32) {
        if self.bubble_time.swap(time, Ordering::Relaxed) != time {
            self.vehicle.entity.send_meta_data(
                &[Metadata::new(
                    pumpkin_data::tracked_data::boat::ID_BUBBLE_TIME,
                    pumpkin_protocol::codec::var_int::VarInt(time),
                )],
                None,
            );
        }
    }

    fn send_wobble_metadata(&self) {
        self.vehicle.send_wobble_metadata();
    }

    /// Vanilla `isClientAuthoritative`: a player rider moves the boat from their client.
    fn is_client_authoritative(&self) -> bool {
        self.get_controlling_passenger().is_some()
    }

    /// Vanilla `isUnderWater`.
    fn is_under_water(&self) -> bool {
        self.status.load().is_some_and(Status::is_under_water)
    }

    const fn max_passengers(&self) -> usize {
        match self.kind {
            BoatKind::Normal => MAX_PASSENGERS,
            BoatKind::Chest(_) => CHEST_BOAT_MAX_PASSENGERS,
        }
    }

    fn passenger_count(&self) -> usize {
        self.vehicle
            .entity
            .passengers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    /// Vanilla `canAddPassenger`.
    fn can_add_passenger(&self) -> bool {
        self.passenger_count() < self.max_passengers()
            && !self.vehicle.entity.is_submerged_in_water()
    }

    /// Vanilla `Entity.startRiding` onto this boat without force.
    fn start_riding(&self, passenger: Arc<dyn EntityBase>) -> bool {
        let rider = passenger.get_entity();
        // Vanilla `canRide`. Players never keep a boarding cooldown.
        let can_ride = passenger.get_player().is_some()
            || (!rider.is_sneaking() && rider.riding_cooldown.load(Ordering::Relaxed) == 0);
        if !can_ride || !self.can_add_passenger() {
            return false;
        }
        let entity = &self.vehicle.entity;
        let Some(vehicle) = entity.world.load().get_entity_by_id(entity.entity_id) else {
            return false;
        };
        let rider_id = rider.entity_id;
        entity.add_passenger(vehicle, passenger);
        entity.has_passenger(rider_id)
    }

    /// Vanilla `Entity.ejectPassengers`: the last passenger first.
    fn eject_passengers(&self) {
        let passengers = self
            .vehicle
            .entity
            .passengers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        for passenger in passengers.iter().rev() {
            self.vehicle
                .entity
                .remove_passenger(passenger.get_entity().entity_id);
        }
    }

    /// Whether the boat's version of `push` moves it and `other` apart.
    fn pushes(&self, other: &Entity) -> bool {
        let own = self.vehicle.entity.bounding_box.load();
        let other_bottom = other.bounding_box.load().min.y;
        if other.entity_type.boat.is_some() {
            other_bottom < own.max.y
        } else {
            other_bottom <= own.min.y
        }
    }

    /// The `tickBubbleColumn` of the server.
    fn tick_bubble_column(&self) {
        if !self.is_above_bubble_column.load(Ordering::Relaxed) {
            self.set_bubble_time(0);
        }
        let mut bubble_time = self.bubble_time.load(Ordering::Relaxed);
        if bubble_time > 0 {
            bubble_time -= 1;
            self.set_bubble_time(bubble_time);
            let diff = BUBBLE_TIME - bubble_time - 1;
            if diff > 0 && bubble_time == 0 {
                self.set_bubble_time(0);
                let drag_down = self.bubble_column_direction_is_down.load(Ordering::Relaxed);
                let entity = &self.vehicle.entity;
                let world = entity.world.load();
                if drag_down {
                    self.eject_passengers();
                    world.broadcast_entity_event(
                        entity,
                        EntityStatus::BoatSink,
                        ActorEventID::None,
                    );
                } else {
                    world.broadcast_entity_event(
                        entity,
                        EntityStatus::BoatLaunch,
                        ActorEventID::None,
                    );
                }
                self.handle_bubble_column_effect(drag_down);
            }
            self.is_above_bubble_column.store(false, Ordering::Relaxed);
        }
    }

    /// Vanilla `handleBubbleColumnEffect`. A player rider's client applies it from the event.
    fn handle_bubble_column_effect(&self, drag_down: bool) {
        if self.is_client_authoritative() {
            return;
        }
        let entity = &self.vehicle.entity;
        let velocity = entity.velocity.load();
        entity.velocity.store(if drag_down {
            velocity.add_raw(0.0, BUBBLE_SINK_SPEED, 0.0)
        } else {
            Vector3::new(velocity.x, BUBBLE_LAUNCH_SPEED, velocity.z)
        });
    }

    /// Vanilla `getPaddleSound`, which checks the status again.
    fn get_paddle_sound(&self) -> Option<Sound> {
        match self.get_status() {
            Status::InWater | Status::UnderWater | Status::UnderFlowingWater => {
                Some(Sound::EntityBoatPaddleWater)
            }
            Status::OnLand => Some(Sound::EntityBoatPaddleLand),
            Status::InAir => None,
        }
    }

    /// The paddle part of vanilla `tick`: a stroke sound every full turn of a moving paddle,
    /// which the rider hears from the server too.
    fn tick_paddles(&self) {
        let entity = &self.vehicle.entity;
        for (side, position) in self.paddle_positions.iter().enumerate() {
            if !self.get_paddle_state(side) {
                position.store(0.0);
                continue;
            }
            let paddle = position.load();
            if !entity.is_silent()
                && paddle % TAU <= PADDLE_SOUND_TIME
                && (paddle + PADDLE_SPEED) % TAU >= PADDLE_SOUND_TIME
                && let Some(sound) = self.get_paddle_sound()
            {
                let view = self.get_looking_vector();
                let (dx, dz) = if side == 1 {
                    (-view.z, view.x)
                } else {
                    (view.z, -view.x)
                };
                let pos = entity.pos.load();
                let pitch = game_random().random::<f32>().mul_add(0.4, 0.8);
                entity.world.load().play_sound_fine(
                    sound,
                    self.sound_source(),
                    &pos.add_raw(dx, 0.0, dz),
                    1.0,
                    pitch,
                );
            }
            position.store(paddle + PADDLE_SPEED);
        }
    }

    /// The end of vanilla `tick`: picks up mobs that bump into an empty seat and pushes
    /// everything else it touches.
    fn push_or_pick_up(&self) {
        let entity = &self.vehicle.entity;
        let world = entity.world.load();
        let area = entity
            .bounding_box
            .load()
            .expand(PUSH_GROWTH, PUSH_HEIGHT_GROWTH, PUSH_GROWTH);
        let own_team = self.get_team();
        // Vanilla `EntitySelector.pushableBy`.
        let touching: Vec<_> = world
            .get_all_at_box(&area)
            .into_iter()
            .filter(|other| {
                other.get_entity().entity_id != entity.entity_id
                    && !other.is_spectator()
                    && other.is_pushable()
                    && is_allowed_by_team_rules(own_team.as_ref(), other.get_team().as_ref())
            })
            .collect();
        let add_new_passengers = !self.is_client_authoritative();
        for other in touching {
            let other_entity = other.get_entity();
            if other_entity.has_passenger(entity.entity_id) {
                continue;
            }
            if add_new_passengers
                && self.passenger_count() < self.max_passengers()
                && !other.is_passenger()
                && other_entity.width() < entity.width()
                && other.get_living_entity().is_some()
                && !other_entity
                    .entity_type
                    .has_tag(&tag::EntityType::MINECRAFT_CANNOT_BE_PUSHED_ONTO_BOATS)
            {
                self.start_riding(other);
            } else if other.get_player().is_some() {
                // The player's own client pushes it; a velocity sent from here would fight it.
                if self.pushes(other_entity) {
                    push_apart(entity, self.is_pushable(), other_entity, false);
                }
            } else {
                self.push(other.as_ref());
            }
        }
    }

    /// Vanilla `calculatePassengerBodyYRot`.
    fn passenger_body_yaw(&self, passenger: &dyn EntityBase) -> f32 {
        let yaw = self.vehicle.entity.yaw.load();
        let is_animal = passenger
            .get_mob()
            .and_then(|mob| mob.as_animal())
            .is_some();
        if is_animal && self.passenger_count() == self.max_passengers() {
            yaw + if passenger.get_entity().entity_id % 2 == 0 {
                90.0
            } else {
                270.0
            }
        } else {
            yaw
        }
    }

    pub fn open_inventory(&self, player: &Arc<Player>) -> bool {
        if let BoatKind::Chest(boat) = &self.kind {
            let custom_name = self.vehicle.entity.custom_name.load().as_ref().clone();
            return boat.interact(
                &self.vehicle.entity,
                custom_name,
                self.get_chest_boat_translation(),
                player,
            );
        }
        false
    }

    fn get_chest_boat_translation(&self) -> TextComponent {
        match self.get_entity().entity_type.id {
            id if id == EntityType::ACACIA_CHEST_BOAT.id => pumpkin_macros::translate_cross!(
                translation::java::ENTITY_MINECRAFT_ACACIA_CHEST_BOAT,
                translation::bedrock::ITEM_CHEST_BOAT_ACACIA_NAME,
            ),
            id if id == EntityType::BIRCH_CHEST_BOAT.id => pumpkin_macros::translate_cross!(
                translation::java::ENTITY_MINECRAFT_BIRCH_CHEST_BOAT,
                translation::bedrock::ITEM_CHEST_BOAT_BIRCH_NAME,
            ),
            id if id == EntityType::DARK_OAK_CHEST_BOAT.id => pumpkin_macros::translate_cross!(
                translation::java::ENTITY_MINECRAFT_DARK_OAK_CHEST_BOAT,
                translation::bedrock::ITEM_CHEST_BOAT_BIG_OAK_NAME,
            ),
            id if id == EntityType::JUNGLE_CHEST_BOAT.id => pumpkin_macros::translate_cross!(
                translation::java::ENTITY_MINECRAFT_JUNGLE_CHEST_BOAT,
                translation::bedrock::ITEM_CHEST_BOAT_JUNGLE_NAME,
            ),
            id if id == EntityType::MANGROVE_CHEST_BOAT.id => pumpkin_macros::translate_cross!(
                translation::java::ENTITY_MINECRAFT_MANGROVE_CHEST_BOAT,
                translation::bedrock::ITEM_CHEST_BOAT_MANGROVE_NAME,
            ),
            id if id == EntityType::OAK_CHEST_BOAT.id => pumpkin_macros::translate_cross!(
                translation::java::ENTITY_MINECRAFT_OAK_CHEST_BOAT,
                translation::bedrock::ITEM_CHEST_BOAT_OAK_NAME,
            ),
            id if id == EntityType::PALE_OAK_CHEST_BOAT.id => pumpkin_macros::translate_cross!(
                translation::java::ENTITY_MINECRAFT_PALE_OAK_CHEST_BOAT,
                translation::bedrock::ITEM_CHEST_BOAT_PALE_OAK_NAME,
            ),
            id if id == EntityType::SPRUCE_CHEST_BOAT.id => pumpkin_macros::translate_cross!(
                translation::java::ENTITY_MINECRAFT_SPRUCE_CHEST_BOAT,
                translation::bedrock::ITEM_CHEST_BOAT_SPRUCE_NAME,
            ),
            id if id == EntityType::BAMBOO_CHEST_RAFT.id => pumpkin_macros::translate_cross!(
                translation::java::ENTITY_MINECRAFT_BAMBOO_CHEST_RAFT,
                translation::bedrock::ITEM_CHEST_BOAT_BAMBOO_NAME,
            ),
            id if id == EntityType::CHERRY_CHEST_BOAT.id => pumpkin_macros::translate_cross!(
                translation::java::ENTITY_MINECRAFT_CHERRY_CHEST_BOAT,
                translation::bedrock::ITEM_CHEST_BOAT_CHERRY_NAME,
            ),
            id if id == EntityType::POPLAR_CHEST_BOAT.id => pumpkin_macros::translate_cross!(
                translation::java::ENTITY_MINECRAFT_POPLAR_CHEST_BOAT,
                // Missing Poplar Bedrock translation
                translation::bedrock::ENTITY_CHEST_BOAT_NAME,
            ),
            _ => pumpkin_macros::translate_cross!(
                translation::java::ENTITY_MINECRAFT_CHEST_BOAT,
                translation::bedrock::ENTITY_CHEST_BOAT_NAME,
            ),
        }
    }
}

impl EntityBase for BoatEntity {
    /// The tick floats and moves the boat by its speed, fluid push, gravity and buoyancy (or
    /// lifts it onto the water it fell into), reads the blocks under and around it, takes the
    /// effects of blocks it touches, ejects its passengers (linked) and pushes or picks up what
    /// touches its slightly grown box. A splash over a bubble column is a game event. Only its
    /// own speed changes for later ticks.
    fn audited_reach(&self) -> Option<AuditedReach> {
        let entity = &self.vehicle.entity;
        let world = entity.world.load();
        // Plugins see every tick through these events, and mounting, dismounting and damage
        // from blocks fire their own.
        if world.server.upgrade().is_some_and(|server| {
            let plugins = &server.plugin_manager;
            plugins.has_handlers::<VehicleUpdateEvent>()
                || plugins.has_handlers::<PacketSentEvent>()
                || plugins.has_handlers::<EntityMountEvent>()
                || plugins.has_handlers::<VehicleEnterEvent>()
                || plugins.has_handlers::<EntityDismountEvent>()
                || plugins.has_handlers::<VehicleExitEvent>()
                || plugins.has_handlers::<VehicleDamageEvent>()
                || plugins.has_handlers::<VehicleDestroyEvent>()
        }) {
            return None;
        }
        let horizontal = entity.velocity.load().length()
            + super::super::max_fluid_push_speed(world.dimension.fast_lava);
        let vertical =
            horizontal + physics::DEFAULT_GRAVITY + MAX_FLOAT_MOVEMENT + self.last_yd.load().abs();
        let motion = (horizontal, vertical);
        let mut links = entity_islands::links_of(self);
        let moved = entity
            .bounding_box
            .load()
            .expand(motion.0, motion.1, motion.0);
        super::super::mob::game_event_listener_links(
            &world,
            self.movement_emission().emits_events(),
            &moved,
            &mut links,
        );
        let mut reach = AuditedReach {
            bounding_box: entity.bounding_box.load(),
            motion,
            entities: PUSH_GROWTH,
            players: PUSH_GROWTH,
            typed_entities: Vec::new(),
            kind: Some(entity.entity_type),
            block_reads: BLOCK_READS,
            route: None,
            changes_blocks: false,
            global: false,
            links,
            sight: Vec::new(),
            spans_contacts: false,
            knockback: 0.0,
        };
        reach.scan_contact_blocks(&world);
        Some(reach)
    }

    fn passenger_attachment_point(
        &self,
        passenger: &dyn EntityBase,
        index: usize,
    ) -> Option<Vector3<f64>> {
        let entity = self.get_entity();
        let seats = entity.entity_type.boat?;
        let mut offset = seats.single_passenger_x_offset;
        let passenger_count = entity
            .passengers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len();
        if passenger_count > 1 {
            offset = if index == 0 {
                FRONT_SEAT_OFFSET
            } else {
                BACK_SEAT_OFFSET
            };
            if passenger
                .get_mob()
                .and_then(|mob| mob.as_animal())
                .is_some()
            {
                offset += ANIMAL_SEAT_SHIFT;
            }
        }
        Some(crate::entity::attachment_point(
            [0.0, seats.ride_height, f64::from(offset)],
            1.0,
            entity.yaw.load(),
        ))
    }

    /// Vanilla `AbstractBoat.getControllingPassenger`: only a player steers a boat.
    fn get_controlling_passenger(&self) -> Option<Arc<dyn EntityBase>> {
        self.get_entity()
            .passengers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .first()
            .filter(|passenger| passenger.get_player().is_some())
            .cloned()
    }

    /// Vanilla `AbstractBoat.positionRider` after the seat: a mob faces where the boat does.
    /// The server never turns a boat from input, so there is no turn to pass on, and players
    /// clamp their own rotation on their client.
    fn on_rider_positioned(&self, passenger: &dyn EntityBase) {
        let rider = passenger.get_entity();
        if rider
            .entity_type
            .has_tag(&tag::EntityType::MINECRAFT_CAN_TURN_IN_BOATS)
        {
            return;
        }
        let Some(mob) = passenger.get_mob() else {
            return;
        };
        let body_yaw = self.passenger_body_yaw(passenger);
        rider.body_yaw.store(body_yaw);
        rider.yaw.store(body_yaw);
        // Vanilla `Mob.clampHeadRotationToBody`.
        let max = mob.get_max_head_rotation();
        let head = rider.head_yaw.load();
        let diff = wrap_degrees(body_yaw - head);
        rider.head_yaw.store(head + diff - diff.clamp(-max, max));
    }

    fn get_entity(&self) -> &Entity {
        &self.vehicle.entity
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        None
    }

    /// Vanilla `AbstractBoat.tick` on the server.
    fn tick(&self, caller: &dyn EntityBase, server: &Server) {
        let entity = &self.vehicle.entity;
        let old_status = self.status.load();
        let status = self.get_status();
        self.status.store(Some(status));
        if status.is_under_water() {
            self.out_of_control_ticks
                .store(self.out_of_control_ticks.load() + 1.0);
        } else {
            self.out_of_control_ticks.store(0.0);
        }
        if self.out_of_control_ticks.load() >= TIME_TO_EJECT {
            self.eject_passengers();
        }

        self.vehicle.tick();
        EntityBase::tick(entity, caller, server);
        if entity.is_removed() {
            return;
        }

        if self.is_client_authoritative() {
            // The rider's client moves it; `handle_move_vehicle` takes the result.
            entity.velocity.store(Vector3::default());
            self.last_yd.store(0.0);
        } else {
            // Only a player rider paddles.
            self.set_paddles(false, false);
            self.float_boat(old_status, status);
            let velocity = entity.velocity.load();
            // Vanilla `checkFallDamage`, which only keeps the speed: boats take no fall damage.
            self.last_yd.store(velocity.y);
            entity.move_entity(caller, velocity);
        }

        // Vanilla applies the block effects twice.
        entity.tick_block_collisions(caller);
        entity.tick_block_collisions(caller);
        self.tick_bubble_column();
        self.tick_paddles();
        self.push_or_pick_up();
    }

    fn get_gravity(&self) -> f64 {
        if self.vehicle.entity.has_no_gravity() {
            0.0
        } else {
            physics::DEFAULT_GRAVITY
        }
    }

    /// Vanilla `AbstractBoat.push`: a boat only pushes boats lower than its top, and other
    /// entities no higher than its bottom.
    fn push(&self, entity: &dyn EntityBase) {
        let other = entity.get_entity();
        if self.pushes(other) {
            push_apart(
                &self.vehicle.entity,
                self.is_pushable(),
                other,
                entity.is_pushable(),
            );
        }
    }

    /// Vanilla `AbstractBoat.onAboveBubbleColumn`: the column sinks or launches the boat once
    /// it has been over it for [`BUBBLE_TIME`] ticks.
    fn on_above_bubble_column(&self, drag_down: bool, _pos: &BlockPos) {
        self.is_above_bubble_column.store(true, Ordering::Relaxed);
        self.bubble_column_direction_is_down
            .store(drag_down, Ordering::Relaxed);
        if self.bubble_time.load(Ordering::Relaxed) == 0 {
            self.set_bubble_time(BUBBLE_TIME);
        }
        // Clients play the splash sound and particles themselves.
        if !self.is_under_water() && game_random().random_range(0..BUBBLE_SPLASH_CHANCE) == 0 {
            let entity = &self.vehicle.entity;
            let controller = self.get_controlling_passenger();
            entity.world.load().emit_game_event_with_context(
                GameEvent::Splash.name(),
                entity.pos.load(),
                &GameEventContext {
                    source_entity: controller.as_deref(),
                },
            );
        }
    }

    fn on_inside_bubble_column(&self, drag_down: bool) {
        // Vanilla `canSimulateMovement`.
        if !self.is_client_authoritative() {
            crate::block::blocks::bubble_column::handle_on_inside_bubble_column(
                &self.vehicle.entity,
                drag_down,
            );
        }
    }

    fn init_data_tracker(&self) {
        self.send_wobble_metadata();
    }

    fn can_hit(&self) -> bool {
        self.vehicle.entity.is_alive()
    }

    fn is_collidable(&self, _entity: Option<Box<dyn EntityBase>>) -> bool {
        true
    }

    fn damage_with_context(
        &self,
        _caller: &dyn EntityBase,
        amount: f32,
        _damage_type: DamageType,
        _position: Option<Vector3<f64>>,
        source: Option<&dyn EntityBase>,
        _cause: Option<&dyn EntityBase>,
    ) -> bool {
        let is_creative = source
            .and_then(EntityBase::get_player)
            .is_some_and(|player| player.gamemode.load() == GameMode::Creative);
        let will_break = self.vehicle.entity.is_alive()
            && (is_creative || self.vehicle.get_damage() + amount * 10.0 > 40.0);

        let damaged = self.vehicle.damage_with_context(amount, source);

        if let BoatKind::Chest(boat) = &self.kind
            && will_break
            && self.vehicle.entity.is_removed()
        {
            let world = self.vehicle.entity.world.load();
            if world.level_info.load().game_rules.entity_drops {
                let position = self.vehicle.entity.block_pos.load();
                boat.inventory().unpack_loot(&self.vehicle.entity, None);
                let inventory: Arc<dyn Inventory> = boat.inventory().clone();
                world.scatter_inventory(&position, &inventory);
            }
        }
        damaged
    }

    /// Vanilla `AbstractBoat.interact`, and `AbstractChestBoat.interact` opening the chest
    /// when the player sneaks or can't board.
    fn interact(&self, player: &Arc<Player>, _item_stack: &mut ItemStack) -> bool {
        let sneaking = player.get_entity().is_sneaking();
        if !sneaking
            && self.out_of_control_ticks.load() < TIME_TO_EJECT
            && !player.get_entity().has_vehicle()
            && let Some(passenger) = self
                .vehicle
                .entity
                .world
                .load()
                .get_player_by_id(player.entity_id())
            && self.start_riding(passenger)
        {
            return true;
        }
        if matches!(self.kind, BoatKind::Chest(_)) && (sneaking || !self.can_add_passenger()) {
            return self.open_inventory(player);
        }
        false
    }

    fn set_paddle_state(&self, left: bool, right: bool) {
        self.set_paddles(left, right);
    }
    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }

    fn is_pushable(&self) -> bool {
        true
    }

    fn write_custom_nbt(&self, nbt: &mut NbtCompound) {
        if let BoatKind::Chest(boat) = &self.kind {
            boat.write_nbt(nbt);
        }
    }

    fn read_custom_nbt(&self, nbt: &NbtCompound) {
        if let BoatKind::Chest(boat) = &self.kind {
            boat.read_nbt(nbt);
        }
    }
}
