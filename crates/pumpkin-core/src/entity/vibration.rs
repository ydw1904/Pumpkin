//! Vanilla `VibrationSystem`: the listener data, selection and travel shared by vibration users.

use std::sync::Arc;

use pumpkin_data::{
    BlockDirection,
    game_event::{GameEvent, PositionSourceType},
    particle::Particle,
    tag::{self, Taggable},
};
use pumpkin_nbt::{compound::NbtCompound, tag::NbtTag};
use pumpkin_protocol::{codec::var_int::VarInt, java::client::play::CParticle, ser::WritingError};
use pumpkin_util::math::{position::BlockPos, vector3::Vector3};
use uuid::Uuid;

use crate::entity::EntityBase;
use crate::world::{World, game_event::GameEventContext};

const PARTICLE_SEND_DISTANCE: f64 = 32.0;

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

    #[cfg(test)]
    pub(crate) fn take_candidate(&mut self) -> Option<(VibrationInfo, i64)> {
        self.candidate.take()
    }

    /// Vanilla `VibrationSelector.addCandidate`.
    // TODO: vanilla breaks distance ties with `VibrationSystem.getGameEventFrequency`.
    pub fn add_candidate(&mut self, vibration: VibrationInfo, time: i64) {
        if self
            .candidate
            .as_ref()
            .is_none_or(|(previous, tick)| *tick == time && vibration.distance < previous.distance)
        {
            self.candidate = Some((vibration, time));
        }
    }

    /// Vanilla `VibrationSystem.Ticker.tick` for a listener at `destination` that moves with
    /// the entity `entity_id`. Returns the vibration that arrived this tick.
    pub fn tick(
        &mut self,
        world: &World,
        entity_id: i32,
        eye_height: f32,
        destination: Vector3<f64>,
    ) -> Option<VibrationInfo> {
        if self.current.is_none()
            && self
                .candidate
                .as_ref()
                .is_some_and(|(_, time)| *time < world.get_world_age())
            && let Some((event, _)) = self.candidate.take()
        {
            self.delay = event.distance.floor() as i32;
            send_vibration_particle(world, event.pos, entity_id, eye_height, self.delay);
            self.current = Some(event);
        }
        let event = self.current.as_ref()?;
        if self.reload_particle {
            let initial = event.distance.floor() as i32;
            let alpha = 1.0 - f64::from(self.delay) / f64::from(initial);
            let pos = event.pos.lerp(&destination, alpha);
            if send_vibration_particle(world, pos, entity_id, eye_height, self.delay) {
                self.reload_particle = false;
            }
        }
        self.delay = (self.delay - 1).max(0);
        if self.delay == 0 {
            return self.current.take();
        }
        None
    }
}

/// Vanilla `VibrationSystem.User.isValidVibration` for a user listening to `listenable`.
#[must_use]
pub fn is_valid_vibration(
    listenable: &tag::Tag,
    event: GameEvent,
    context: &GameEventContext<'_>,
) -> bool {
    if !listenable.0.contains(&event.name()) {
        return false;
    }
    let Some(source) = context.source_entity else {
        return true;
    };
    let ignored_while_sneaking = tag::GameEvent::MINECRAFT_IGNORE_VIBRATIONS_SNEAKING
        .0
        .contains(&event.name());
    !source.is_spectator()
        && (!source.get_entity().is_sneaking() || !ignored_while_sneaking)
        && !source.dampens_vibrations()
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
    entity_id: i32,
    eye_height: f32,
    delay: i32,
) -> bool {
    let Ok(payload) = vibration_payload(entity_id, eye_height, delay) else {
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

fn vibration_payload(entity_id: i32, eye_offset: f32, delay: i32) -> Result<Vec<u8>, WritingError> {
    let mut payload = Vec::new();
    VarInt(PositionSourceType::Entity.to_id()).encode(&mut payload)?;
    VarInt(entity_id).encode(&mut payload)?;
    payload.extend_from_slice(&eye_offset.to_be_bytes());
    VarInt(delay).encode(&mut payload)?;
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entity_destination_particle_matches_vanilla_stream_codec() -> Result<(), WritingError> {
        assert_eq!(
            vibration_payload(300, 0.36, 12)?,
            vec![1, 172, 2, 62, 184, 81, 236, 12]
        );
        Ok(())
    }
}
