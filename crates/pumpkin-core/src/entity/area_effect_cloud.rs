use std::collections::HashMap;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::entity::living::EffectSource;
use crate::entity::projectile::owner::EntityReference;
use crate::item::potion::{OPAQUE, PotionContents};
use crate::{
    entity::{Entity, EntityBase},
    server::Server,
};
use pumpkin_data::data_component_impl::{
    PotionContentsImpl, PotionDurationScaleImpl, StatusEffectInstance,
};
use pumpkin_data::entity::EntityType;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::particle::Particle;
use pumpkin_data::tracked_data::area_effect_cloud::{PARTICLE, RADIUS, WAITING};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::math::boundingbox::EntityDimensions;

const TIME_BETWEEN_APPLICATIONS: i32 = 5;
const MAX_RADIUS: f32 = 32.0;
const MINIMAL_RADIUS: f32 = 0.5;
const DEFAULT_RADIUS: f32 = 3.0;
const HEIGHT: f32 = 0.5;
pub const INFINITE_DURATION: i32 = -1;
const DEFAULT_WAIT_TIME: i32 = 20;
const DEFAULT_REAPPLICATION_DELAY: i32 = 20;
/// The potency vanilla applies instant effects with from a cloud.
const INSTANT_EFFECT_SCALE: f64 = 0.5;

#[derive(Clone)]
struct ParticleMeta {
    particle_id: VarInt,
    data: Box<[u8]>,
}

impl pumpkin_protocol::java::client::play::MetadataSerializer for ParticleMeta {
    fn write_metadata(
        &self,
        writer: &mut impl std::io::Write,
        _version: &pumpkin_util::version::JavaMinecraftVersion,
    ) -> Result<(), pumpkin_protocol::ser::WritingError> {
        use pumpkin_protocol::ser::NetworkWriteExt;
        writer.write_var_int(&self.particle_id)?;
        writer.write_slice(&self.data)
    }
}

/// A cloud's `custom_particle`. Only the dragon's breath particle vanilla itself uses is supported.
#[derive(Clone, Copy, PartialEq)]
pub enum CloudParticle {
    /// `PowerParticleOption` of `minecraft:dragon_breath`.
    DragonBreath { power: f32 },
}

impl CloudParticle {
    fn write_nbt(self) -> NbtCompound {
        let mut nbt = NbtCompound::new();
        match self {
            Self::DragonBreath { power } => {
                nbt.put_string("type", "minecraft:dragon_breath".to_string());
                nbt.put_float("power", power);
            }
        }
        nbt
    }

    fn read_nbt(nbt: &NbtCompound) -> Option<Self> {
        let name = nbt.get_string("type")?;
        match name.strip_prefix("minecraft:").unwrap_or(name) {
            "dragon_breath" => Some(Self::DragonBreath {
                power: nbt.get_float("power").unwrap_or(1.0),
            }),
            _ => None,
        }
    }
}

struct CloudState {
    radius: f32,
    waiting: bool,
    custom_particle: Option<CloudParticle>,
    potion_contents: Option<PotionContentsImpl>,
    potion_duration_scale: f32,
    /// Entity id to the tick count from which it can be affected again.
    victims: HashMap<i32, i32>,
    duration: i32,
    wait_time: i32,
    reapplication_delay: i32,
    duration_on_use: i32,
    radius_on_use: f32,
    radius_per_tick: f32,
}

/// Vanilla `AreaEffectCloud`, left by lingering potions, creepers with effects and the ender dragon.
pub struct AreaEffectCloudEntity {
    pub entity: Entity,
    owner: EntityReference,
    state: Mutex<CloudState>,
}

impl AreaEffectCloudEntity {
    #[must_use]
    pub fn new(entity: Entity) -> Arc<Self> {
        entity.no_physics.store(true, Relaxed);
        let cloud = Self {
            entity,
            owner: EntityReference::none(),
            state: Mutex::new(CloudState {
                radius: DEFAULT_RADIUS,
                waiting: false,
                custom_particle: None,
                potion_contents: None,
                potion_duration_scale: 1.0,
                victims: HashMap::new(),
                duration: INFINITE_DURATION,
                wait_time: DEFAULT_WAIT_TIME,
                reapplication_delay: DEFAULT_REAPPLICATION_DELAY,
                duration_on_use: 0,
                radius_on_use: 0.0,
                radius_per_tick: 0.0,
            }),
        };
        cloud.set_radius(DEFAULT_RADIUS);
        cloud.update_particle();
        Arc::new(cloud)
    }

    fn state(&self) -> MutexGuard<'_, CloudState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    #[must_use]
    pub fn radius(&self) -> f32 {
        self.state().radius
    }

    /// Vanilla `setRadius`: clamped, synced, and resizes the bounding box.
    pub fn set_radius(&self, radius: f32) {
        let radius = radius.clamp(0.0, MAX_RADIUS);
        self.state().radius = radius;
        self.entity.set_synced_data(RADIUS, radius);
        let eye_height = self.entity.entity_type.eye_height;
        self.entity
            .set_default_dimensions(EntityDimensions::new(radius * 2.0, HEIGHT, eye_height));
    }

    pub fn set_radius_on_use(&self, radius_on_use: f32) {
        self.state().radius_on_use = radius_on_use;
    }

    pub fn set_radius_per_tick(&self, radius_per_tick: f32) {
        self.state().radius_per_tick = radius_per_tick;
    }

    #[must_use]
    pub fn duration(&self) -> i32 {
        self.state().duration
    }

    pub fn set_duration(&self, duration: i32) {
        self.state().duration = duration;
    }

    pub fn set_wait_time(&self, wait_time: i32) {
        self.state().wait_time = wait_time;
    }

    pub fn set_potion_duration_scale(&self, scale: f32) {
        self.state().potion_duration_scale = scale;
    }

    pub fn set_potion_contents(&self, contents: Option<PotionContentsImpl>) {
        self.state().potion_contents = contents;
        self.update_particle();
    }

    pub fn set_custom_particle(&self, particle: Option<CloudParticle>) {
        self.state().custom_particle = particle;
        self.update_particle();
    }

    /// Vanilla `addEffect`: appends a custom effect to the potion contents.
    pub fn add_effect(&self, effect: StatusEffectInstance) {
        self.state()
            .potion_contents
            .get_or_insert_with(|| PotionContentsImpl {
                potion_id: None,
                custom_color: None,
                custom_effects: Vec::new(),
                custom_name: None,
            })
            .custom_effects
            .push(effect);
        self.update_particle();
    }

    /// Vanilla `applyComponentsFromItemStack` for the potion components a cloud reads.
    pub fn apply_components_from_item_stack(&self, stack: &ItemStack) {
        if let Some(contents) = stack.get_data_component::<PotionContentsImpl>() {
            self.set_potion_contents(Some(contents.clone()));
        }
        if let Some(scale) = stack.get_data_component::<PotionDurationScaleImpl>() {
            self.set_potion_duration_scale(scale.scale);
        }
    }

    pub fn set_owner(&self, owner: Option<&Entity>) {
        self.owner.set(owner);
    }

    /// Vanilla `getOwner`: only a living owner counts.
    #[must_use]
    pub fn owner(&self) -> Option<Arc<dyn EntityBase>> {
        self.owner
            .get(&self.entity.world.load())
            .filter(|owner| owner.get_living_entity().is_some())
    }

    fn update_particle(&self) {
        let state = self.state();
        let meta = match state.custom_particle {
            Some(CloudParticle::DragonBreath { power }) => ParticleMeta {
                particle_id: VarInt(Particle::DragonBreath as i32),
                data: Box::new(power.to_be_bytes()),
            },
            None => {
                let color = PotionContents::color(state.potion_contents.as_ref()) | OPAQUE;
                ParticleMeta {
                    particle_id: VarInt(Particle::EntityEffect as i32),
                    data: Box::new(color.to_be_bytes()),
                }
            }
        };
        drop(state);
        self.entity.set_synced_data(PARTICLE, meta);
    }

    /// Vanilla `serverTick`.
    #[expect(clippy::too_many_lines, reason = "mirrors vanilla's single method")]
    fn server_tick(&self) {
        let tick_count = self.entity.tick_count.load(Relaxed);
        let mut state = self.state();
        if state.duration != INFINITE_DURATION && tick_count - state.wait_time >= state.duration {
            drop(state);
            self.entity.remove();
            return;
        }

        let should_wait = tick_count < state.wait_time;
        if state.waiting != should_wait {
            state.waiting = should_wait;
            self.entity.set_synced_data(WAITING, should_wait);
        }
        if should_wait {
            return;
        }

        let mut radius = state.radius;
        if state.radius_per_tick != 0.0 {
            radius += state.radius_per_tick;
            if radius < MINIMAL_RADIUS {
                drop(state);
                self.entity.remove();
                return;
            }
            drop(state);
            self.set_radius(radius);
            state = self.state();
        }

        if tick_count % TIME_BETWEEN_APPLICATIONS != 0 {
            return;
        }
        state.victims.retain(|_, until| tick_count < *until);
        let effects = state
            .potion_contents
            .as_ref()
            .map(PotionContents::effects_of)
            .unwrap_or_default();
        if effects.is_empty() {
            state.victims.clear();
            return;
        }
        let duration_scale = state.potion_duration_scale;
        let reapplication_delay = state.reapplication_delay;
        let effects: Vec<_> = effects
            .into_iter()
            .map(|mut effect| {
                // `MobEffectInstance.withScaledDuration`.
                if effect.duration != INFINITE_DURATION && effect.duration != 0 {
                    effect.duration =
                        ((effect.duration as f32 * duration_scale).floor() as i32).max(1);
                }
                effect
            })
            .collect();
        drop(state);

        let world = self.entity.world.load();
        let bounding_box = self.entity.bounding_box.load();
        let pos = self.entity.pos.load();
        let owner = self.owner();
        for candidate in world.get_entities_and_players_at_box(&bounding_box) {
            let Some(living) = candidate.get_living_entity() else {
                continue;
            };
            let candidate_entity = candidate.get_entity();
            // `isAffectedByPotions`, and `getEntitiesOfClass` skipping spectators.
            if candidate.is_spectator()
                || living.is_dead_or_dying()
                || candidate_entity.entity_type == &EntityType::ARMOR_STAND
                || !effects.iter().any(|effect| living.can_be_affected(effect))
                || self
                    .state()
                    .victims
                    .contains_key(&candidate_entity.entity_id)
            {
                continue;
            }
            let candidate_pos = candidate_entity.pos.load();
            let dx = candidate_pos.x - pos.x;
            let dz = candidate_pos.z - pos.z;
            if dx * dx + dz * dz > f64::from(radius * radius) {
                continue;
            }

            self.state()
                .victims
                .insert(candidate_entity.entity_id, tick_count + reapplication_delay);
            for effect in &effects {
                if !living.apply_instant_effect(
                    effect.effect_type,
                    effect.amplifier,
                    Some(INSTANT_EFFECT_SCALE),
                    Some(EffectSource {
                        direct: self,
                        owner: owner.as_deref(),
                    }),
                ) {
                    living.add_effect_with_source(effect.clone(), Some(self));
                }
            }

            let mut state = self.state();
            if state.radius_on_use != 0.0 {
                radius += state.radius_on_use;
                if radius < MINIMAL_RADIUS {
                    drop(state);
                    self.entity.remove();
                    return;
                }
                drop(state);
                self.set_radius(radius);
                state = self.state();
            }
            if state.duration_on_use != 0 && state.duration != INFINITE_DURATION {
                state.duration += state.duration_on_use;
                if state.duration <= 0 {
                    drop(state);
                    self.entity.remove();
                    return;
                }
            }
        }
    }
}

impl EntityBase for AreaEffectCloudEntity {
    fn init_data_tracker(&self) {
        let (radius, waiting) = {
            let state = self.state();
            (state.radius, state.waiting)
        };
        self.entity.set_synced_data(RADIUS, radius);
        self.entity.set_synced_data(WAITING, waiting);
        self.update_particle();
    }

    fn tick(&self, caller: &dyn EntityBase, server: &Server) {
        self.entity.tick(caller, server);
        if self.entity.is_removed() {
            return;
        }
        self.server_tick();
    }

    fn get_owner_entity(&self) -> Option<Arc<dyn EntityBase>> {
        self.owner()
    }

    fn write_custom_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_int("Age", self.entity.tick_count.load(Relaxed));
        let state = self.state();
        nbt.put_int("Duration", state.duration);
        nbt.put_int("WaitTime", state.wait_time);
        nbt.put_int("ReapplicationDelay", state.reapplication_delay);
        nbt.put_int("DurationOnUse", state.duration_on_use);
        nbt.put_float("RadiusOnUse", state.radius_on_use);
        nbt.put_float("RadiusPerTick", state.radius_per_tick);
        nbt.put_float("Radius", state.radius);
        if let Some(particle) = state.custom_particle {
            nbt.put_compound("custom_particle", particle.write_nbt());
        }
        self.owner.write_nbt(nbt, "Owner");
        if let Some(contents) = &state.potion_contents {
            use pumpkin_data::data_component_impl::DataComponentImpl;
            nbt.put("potion_contents", contents.write_data());
        }
        if state.potion_duration_scale != 1.0 {
            nbt.put_float("potion_duration_scale", state.potion_duration_scale);
        }
    }

    /// Updates only the keys present, so a partial read keeps the rest.
    fn read_custom_nbt(&self, nbt: &NbtCompound) {
        if let Some(age) = nbt.get_int("Age") {
            self.entity.tick_count.store(age, Relaxed);
        }
        {
            let mut state = self.state();
            if let Some(duration) = nbt.get_int("Duration") {
                state.duration = duration;
            }
            if let Some(wait_time) = nbt.get_int("WaitTime") {
                state.wait_time = wait_time;
            }
            if let Some(delay) = nbt.get_int("ReapplicationDelay") {
                state.reapplication_delay = delay;
            }
            if let Some(duration_on_use) = nbt.get_int("DurationOnUse") {
                state.duration_on_use = duration_on_use;
            }
            if let Some(radius_on_use) = nbt.get_float("RadiusOnUse") {
                state.radius_on_use = radius_on_use;
            }
            if let Some(radius_per_tick) = nbt.get_float("RadiusPerTick") {
                state.radius_per_tick = radius_per_tick;
            }
            if let Some(scale) = nbt.get_float("potion_duration_scale") {
                state.potion_duration_scale = scale;
            }
        }
        if let Some(radius) = nbt.get_float("Radius") {
            self.set_radius(radius);
        }
        self.owner.read_nbt(nbt, "Owner");
        if let Some(particle) = nbt.get_compound("custom_particle") {
            self.set_custom_particle(CloudParticle::read_nbt(particle));
        }
        if let Some(contents) = nbt.get("potion_contents") {
            self.set_potion_contents(PotionContentsImpl::read_data(contents));
        }
    }

    fn get_entity(&self) -> &Entity {
        &self.entity
    }

    fn get_living_entity(&self) -> Option<&crate::entity::living::LivingEntity> {
        None
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }
}
