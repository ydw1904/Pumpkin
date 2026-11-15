//! Vanilla `VibrationSystem`: the listener data, selection and travel shared by vibration users.

use std::sync::Arc;

use pumpkin_data::{
    Block, BlockDirection,
    game_event::{GameEvent, PositionSourceType},
    particle::Particle,
    tag::{self, Taggable},
};
use pumpkin_nbt::{compound::NbtCompound, tag::NbtTag};
use pumpkin_protocol::{codec::var_int::VarInt, java::client::play::CParticle, ser::WritingError};
use pumpkin_util::math::{position::BlockPos, vector2::Vector2, vector3::Vector3};
use uuid::Uuid;

use crate::entity::EntityBase;
use crate::entity::player::advancement::trigger::AdvancementTrigger;
use crate::world::{World, game_event::GameEventContext};

const PARTICLE_SEND_DISTANCE: f64 = 32.0;

/// Vanilla `PositionSource` of a vibration listener, the target of its vibration particles.
#[derive(Clone, Copy)]
pub enum ListenerSource {
    Entity { id: i32, eye_height: f32 },
    Block(BlockPos),
}

/// Vanilla `VibrationSystem.getGameEventFrequency`, 0 for events without one.
#[must_use]
pub const fn game_event_frequency(event: GameEvent) -> i32 {
    match event {
        GameEvent::Step | GameEvent::Swim | GameEvent::Flap | GameEvent::Resonate1 => 1,
        GameEvent::ProjectileLand
        | GameEvent::HitGround
        | GameEvent::Splash
        | GameEvent::Bounce
        | GameEvent::Resonate2 => 2,
        GameEvent::ItemInteractFinish
        | GameEvent::ProjectileShoot
        | GameEvent::InstrumentPlay
        | GameEvent::Resonate3 => 3,
        GameEvent::EntityAction
        | GameEvent::ElytraGlide
        | GameEvent::Unequip
        | GameEvent::Resonate4 => 4,
        GameEvent::EntityDismount | GameEvent::Equip | GameEvent::Resonate5 => 5,
        GameEvent::EntityInteract
        | GameEvent::Shear
        | GameEvent::EntityMount
        | GameEvent::Resonate6 => 6,
        GameEvent::EntityDamage | GameEvent::Resonate7 => 7,
        GameEvent::Drink | GameEvent::Eat | GameEvent::Resonate8 => 8,
        GameEvent::ContainerClose
        | GameEvent::BlockClose
        | GameEvent::BlockDeactivate
        | GameEvent::BlockDetach
        | GameEvent::Resonate9 => 9,
        GameEvent::ContainerOpen
        | GameEvent::BlockOpen
        | GameEvent::BlockActivate
        | GameEvent::BlockAttach
        | GameEvent::PrimeFuse
        | GameEvent::NoteBlockPlay
        | GameEvent::Resonate10 => 10,
        GameEvent::BlockChange | GameEvent::Resonate11 => 11,
        GameEvent::BlockDestroy | GameEvent::FluidPickup | GameEvent::Resonate12 => 12,
        GameEvent::BlockPlace | GameEvent::FluidPlace | GameEvent::Resonate13 => 13,
        GameEvent::EntityPlace
        | GameEvent::LightningStrike
        | GameEvent::Teleport
        | GameEvent::Resonate14 => 14,
        GameEvent::EntityDie | GameEvent::Explode | GameEvent::Resonate15 => 15,
        _ => 0,
    }
}

/// Vanilla `VibrationSystem.getResonanceEventByFrequency`, for frequencies 1 to 15.
#[must_use]
pub const fn resonance_event(frequency: i32) -> Option<GameEvent> {
    Some(match frequency {
        1 => GameEvent::Resonate1,
        2 => GameEvent::Resonate2,
        3 => GameEvent::Resonate3,
        4 => GameEvent::Resonate4,
        5 => GameEvent::Resonate5,
        6 => GameEvent::Resonate6,
        7 => GameEvent::Resonate7,
        8 => GameEvent::Resonate8,
        9 => GameEvent::Resonate9,
        10 => GameEvent::Resonate10,
        11 => GameEvent::Resonate11,
        12 => GameEvent::Resonate12,
        13 => GameEvent::Resonate13,
        14 => GameEvent::Resonate14,
        15 => GameEvent::Resonate15,
        _ => return None,
    })
}

/// Vanilla `VibrationSystem.getRedstoneStrengthForDistance`.
#[must_use]
pub fn redstone_strength_for_distance(distance: f32, listener_radius: i32) -> u8 {
    let power_scale = 15.0 / f64::from(listener_radius);
    (15 - (power_scale * f64::from(distance)).floor() as i32).clamp(1, 15) as u8
}

/// Vanilla `VibrationSystem.Ticker.areAdjacentChunksTicking`.
#[must_use]
pub fn adjacent_chunks_ticking(world: &World, listener: &BlockPos) -> bool {
    let center = listener.chunk_position();
    let active_chunks = world
        .active_chunks
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    (-1..=1).all(|x| {
        (-1..=1).all(|z| {
            let chunk = Vector2::new(center.x + x, center.y + z);
            active_chunks.block_ticking.contains(&chunk) && world.level.is_chunk_loaded(&chunk)
        })
    })
}

/// Vanilla `VibrationInfo`.
#[derive(Clone)]
pub struct VibrationInfo {
    pub event: GameEvent,
    pub distance: f32,
    pub pos: Vector3<f64>,
    source: Option<Uuid>,
    projectile_owner: Option<Uuid>,
}

impl VibrationInfo {
    #[must_use]
    pub fn new(
        event: GameEvent,
        distance: f32,
        pos: Vector3<f64>,
        source: Option<&dyn EntityBase>,
    ) -> Self {
        let world = source.map(|source| source.get_entity().world.load_full());
        Self {
            event,
            distance,
            pos,
            source: source.map(|source| source.get_entity().entity_uuid),
            projectile_owner: source.zip(world).and_then(|(source, world)| {
                source
                    .get_projectile_owner()?
                    .owner()
                    .get(&world)
                    .map(|owner| owner.get_entity().entity_uuid)
            }),
        }
    }

    /// Vanilla `VibrationInfo.getEntity`.
    #[must_use]
    pub fn source_entity(&self, world: &World) -> Option<Arc<dyn EntityBase>> {
        let uuid = self.source?;
        world
            .get_player_by_uuid(uuid)
            .map(|player| player as Arc<dyn EntityBase>)
            .or_else(|| world.get_entity_by_uuid(uuid))
    }

    /// Vanilla `VibrationInfo.getProjectileOwner`.
    #[must_use]
    pub fn projectile_owner(&self, world: &World) -> Option<Arc<dyn EntityBase>> {
        self.source_entity(world)
            .and_then(|source| source.get_projectile_owner()?.owner().get(world))
            .or_else(|| {
                let uuid = self.projectile_owner?;
                world
                    .get_player_by_uuid(uuid)
                    .map(|player| player as Arc<dyn EntityBase>)
                    .or_else(|| world.get_entity_by_uuid(uuid))
            })
    }

    fn read_nbt(nbt: &NbtCompound) -> Option<Self> {
        let event = GameEvent::from_name(nbt.get_string("game_event")?)?;
        let distance = nbt.get_float("distance")?;
        let pos = nbt.get_list("pos")?;
        if pos.len() != 3 || !distance.is_finite() || distance < 0.0 {
            return None;
        }
        let pos = Vector3::new(
            pos[0].extract_double()?,
            pos[1].extract_double()?,
            pos[2].extract_double()?,
        );
        if !pos.x.is_finite() || !pos.y.is_finite() || !pos.z.is_finite() {
            return None;
        }
        Some(Self {
            event,
            distance,
            pos,
            source: nbt.get_uuid("source"),
            projectile_owner: nbt.get_uuid("projectile_owner"),
        })
    }

    fn write_nbt(&self) -> NbtCompound {
        let mut nbt = NbtCompound::new();
        nbt.put_string("game_event", format!("minecraft:{}", self.event.name()));
        nbt.put_float("distance", self.distance);
        nbt.put_list(
            "pos",
            vec![
                NbtTag::Double(self.pos.x),
                NbtTag::Double(self.pos.y),
                NbtTag::Double(self.pos.z),
            ],
        );
        if let Some(source) = self.source {
            nbt.put_uuid("source", source);
        }
        if let Some(owner) = self.projectile_owner {
            nbt.put_uuid("projectile_owner", owner);
        }
        nbt
    }
}

/// Vanilla `VibrationSystem.Data` with its `VibrationSelector`.
#[derive(Default)]
pub struct VibrationData {
    current: Option<VibrationInfo>,
    candidate: Option<(VibrationInfo, i64)>,
    delay: i32,
    reload_particle: bool,
}

impl VibrationData {
    #[must_use]
    pub fn read_nbt(nbt: &NbtCompound) -> Self {
        let Some(selector) = nbt.get_compound("selector") else {
            return Self::default();
        };
        let Some(tick) = selector.get_long("tick") else {
            return Self::default();
        };
        let delay = nbt.get_int("event_delay").unwrap_or(0);
        if delay < 0 {
            return Self::default();
        }
        Self {
            current: nbt.get_compound("event").and_then(VibrationInfo::read_nbt),
            candidate: selector
                .get_compound("event")
                .and_then(VibrationInfo::read_nbt)
                .map(|event| (event, tick)),
            delay,
            reload_particle: true,
        }
    }

    #[must_use]
    pub fn write_nbt(&self) -> NbtCompound {
        let mut nbt = NbtCompound::new();
        if let Some(event) = &self.current {
            nbt.put_compound("event", event.write_nbt());
        }
        let mut selector = NbtCompound::new();
        selector.put_long(
            "tick",
            self.candidate.as_ref().map_or(-1, |(_, tick)| *tick),
        );
        if let Some((event, _)) = &self.candidate {
            selector.put_compound("event", event.write_nbt());
        }
        nbt.put_compound("selector", selector);
        nbt.put_int("event_delay", self.delay);
        nbt
    }

    #[must_use]
    pub const fn has_current(&self) -> bool {
        self.current.is_some()
    }

    /// Whether the next `tick` can deliver a vibration.
    #[must_use]
    pub fn may_arrive_next_tick(&self) -> bool {
        self.current
            .as_ref()
            .map_or(self.candidate.is_some(), |_| self.delay <= 1)
    }

    #[cfg(test)]
    pub(crate) const fn take_candidate(&mut self) -> Option<(VibrationInfo, i64)> {
        self.candidate.take()
    }

    /// Vanilla `VibrationSelector.addCandidate`: the closest vibration of the tick wins, then the
    /// highest frequency.
    pub fn add_candidate(&mut self, vibration: VibrationInfo, time: i64) {
        if self.candidate.as_ref().is_none_or(|(previous, tick)| {
            *tick == time
                && (vibration.distance < previous.distance
                    || (vibration.distance == previous.distance
                        && game_event_frequency(vibration.event)
                            > game_event_frequency(previous.event)))
        }) {
            self.candidate = Some((vibration, time));
        }
    }

    /// Vanilla `VibrationSystem.Ticker.tick` for a listener at `destination`. Returns the
    /// vibration that arrived this tick. `can_receive` is checked on arrival: while it fails the
    /// vibration waits, like vanilla `requiresAdjacentChunksToBeTicking`.
    pub fn tick(
        &mut self,
        world: &World,
        source: ListenerSource,
        destination: Vector3<f64>,
        can_receive: impl FnOnce() -> bool,
    ) -> Option<VibrationInfo> {
        if self.current.is_none()
            && self
                .candidate
                .as_ref()
                .is_some_and(|(_, time)| *time < world.get_world_age())
            && let Some((event, _)) = self.candidate.take()
        {
            self.delay = event.distance.floor() as i32;
            send_vibration_particle(world, event.pos, source, self.delay);
            self.current = Some(event);
        }
        let event = self.current.as_ref()?;
        if self.reload_particle {
            let initial = event.distance.floor() as i32;
            let alpha = 1.0 - f64::from(self.delay) / f64::from(initial);
            let pos = event.pos.lerp(&destination, alpha);
            if send_vibration_particle(world, pos, source, self.delay) {
                self.reload_particle = false;
            }
        }
        self.delay = (self.delay - 1).max(0);
        if self.delay == 0 && can_receive() {
            return self.current.take();
        }
        None
    }
}

/// Vanilla `VibrationSystem.User.isValidVibration` for a user listening to `listenable`. A
/// user that `can_trigger_avoid_vibration` grants `minecraft:avoid_vibration` to a player
/// sneaking past it.
#[must_use]
pub fn is_valid_vibration(
    listenable: &tag::Tag,
    event: GameEvent,
    context: &GameEventContext<'_>,
    can_trigger_avoid_vibration: bool,
) -> bool {
    if !listenable.0.contains(&event.name()) {
        return false;
    }
    if let Some(source) = context.source_entity {
        if source.is_spectator() {
            return false;
        }
        if source.is_stepping_carefully()
            && tag::GameEvent::MINECRAFT_IGNORE_VIBRATIONS_SNEAKING
                .0
                .contains(&event.name())
        {
            if can_trigger_avoid_vibration && let Some(player) = source.get_player() {
                player.trigger_advancement(AdvancementTrigger::AvoidVibration);
            }
            return false;
        }
        if source.dampens_vibrations() {
            return false;
        }
    }
    context.affected_state.is_none_or(|state| {
        !Block::from_state_id(state).has_tag(&tag::Block::MINECRAFT_DAMPENS_VIBRATIONS)
    })
}

/// Vanilla `VibrationSystem.Listener.isOccluded`.
#[must_use]
pub fn is_occluded(world: &World, origin: Vector3<f64>, destination: Vector3<f64>) -> bool {
    let from = BlockPos::floored_v(origin).to_centered_f64();
    let to = BlockPos::floored_v(destination).to_centered_f64();
    BlockDirection::all().iter().all(|direction| {
        let nudged_source = from + direction.to_offset().to_f64() * f64::from(1.0e-5f32);
        pumpkin_world::clip::traverse_blocks(nudged_source, to, |pos| {
            world
                .get_block(pos)
                .has_tag(&tag::Block::MINECRAFT_OCCLUDES_VIBRATION_SIGNALS)
                .then_some(())
        })
        .is_some()
    })
}

fn send_vibration_particle(
    world: &World,
    pos: Vector3<f64>,
    source: ListenerSource,
    delay: i32,
) -> bool {
    let Ok(payload) = vibration_payload(source, delay) else {
        return false;
    };
    let packet = CParticle::new(
        false,
        false,
        pos,
        Vector3::new(0.0, 0.0, 0.0),
        0.0,
        1,
        VarInt(i32::from(Particle::Vibration.to_id())),
        &payload,
    );
    let mut sent = false;
    for player in world.players.load().iter() {
        if player.position().squared_distance_to_vec(&pos)
            <= PARTICLE_SEND_DISTANCE * PARTICLE_SEND_DISTANCE
        {
            player.try_send_client_packet(&packet);
            sent = true;
        }
    }
    sent
}

/// Vanilla `VibrationParticleOption.STREAM_CODEC`: the position source, then the arrival ticks.
fn vibration_payload(source: ListenerSource, delay: i32) -> Result<Vec<u8>, WritingError> {
    let mut payload = Vec::new();
    match source {
        ListenerSource::Entity { id, eye_height } => {
            VarInt(PositionSourceType::Entity.to_id()).encode(&mut payload)?;
            VarInt(id).encode(&mut payload)?;
            payload.extend_from_slice(&eye_height.to_be_bytes());
        }
        ListenerSource::Block(pos) => {
            VarInt(PositionSourceType::Block.to_id()).encode(&mut payload)?;
            payload.extend_from_slice(&pos.as_long().to_be_bytes());
        }
    }
    VarInt(delay).encode(&mut payload)?;
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entity_destination_particle_matches_vanilla_stream_codec() -> Result<(), WritingError> {
        assert_eq!(
            vibration_payload(
                ListenerSource::Entity {
                    id: 300,
                    eye_height: 0.36
                },
                12
            )?,
            vec![1, 172, 2, 62, 184, 81, 236, 12]
        );
        Ok(())
    }

    #[test]
    fn equally_close_vibrations_pick_the_higher_frequency() {
        let at = Vector3::new(0.0, 64.0, 0.0);
        let mut data = VibrationData::default();
        data.add_candidate(VibrationInfo::new(GameEvent::BlockPlace, 4.0, at, None), 7);
        data.add_candidate(VibrationInfo::new(GameEvent::Step, 4.0, at, None), 7);
        data.add_candidate(VibrationInfo::new(GameEvent::Explode, 4.0, at, None), 7);
        // A closer one of a later tick doesn't replace it.
        data.add_candidate(VibrationInfo::new(GameEvent::EntityDie, 1.0, at, None), 8);
        assert!(
            data.take_candidate()
                .is_some_and(|(event, tick)| event.event == GameEvent::Explode && tick == 7)
        );
    }
}
