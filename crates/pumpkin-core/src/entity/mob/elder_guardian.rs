use crate::entity::r#type::CreatureSpawnLevel;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Weak};

use pumpkin_data::damage::DamageType;
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::potion::Effect;
use pumpkin_protocol::java::client::play::{CGameEvent, GameEvent};
use pumpkin_util::GameMode;

use crate::entity::mob::guardian::{
    GuardianState, guardian_ai_step, guardian_pre_damage, guardian_travel_in_water,
    guardian_walk_target_value, init_guardian,
};
use crate::entity::{
    Entity, EntityBase,
    mob::{Mob, MobEntity},
};

/// Vanilla `ElderGuardian.getAttackDuration`.
pub const ATTACK_DURATION: i32 = 60;
const RANDOM_STROLL_INTERVAL: i32 = 400;
const EFFECT_INTERVAL: i32 = 1200;
const EFFECT_RADIUS: f64 = 50.0;
const EFFECT_DURATION: i32 = 6000;
const EFFECT_AMPLIFIER: u8 = 2;
const EFFECT_DISPLAY_LIMIT: i32 = 1200;
const HOME_RADIUS: i32 = 16;

pub struct ElderGuardianEntity {
    pub mob_entity: MobEntity,
    state: Arc<GuardianState>,
}

impl ElderGuardianEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let state = Arc::new(GuardianState::default());
        Arc::new_cyclic(|weak: &Weak<Self>| {
            let mob_weak: Weak<dyn Mob> = weak.clone();
            let mut mob_entity = MobEntity::new(entity);
            init_guardian(
                &mut mob_entity,
                &state,
                &mob_weak,
                true,
                RANDOM_STROLL_INTERVAL,
            );
            mob_entity
                .persistence_required
                .store(true, Ordering::Relaxed);
            Self { mob_entity, state }
        })
    }

    /// Vanilla `MobEffectUtil.addEffectToPlayersAround` with the elder guardian's mining fatigue.
    fn apply_mining_fatigue(&self) {
        let entity = &self.mob_entity.living_entity.entity;
        let world = entity.world.load();
        let packet = CGameEvent::new(
            GameEvent::PlayElderGuardianMobAppearance,
            if entity.is_silent() { 0.0 } else { 1.0 },
        );
        for player in world.get_nearby_players(entity.pos.load(), EFFECT_RADIUS) {
            if !matches!(
                player.gamemode.load(),
                GameMode::Survival | GameMode::Adventure
            ) || (self as &dyn EntityBase).is_allied_to(player.as_ref())
            {
                continue;
            }
            let current = player
                .living_entity
                .get_effect(&StatusEffect::MINING_FATIGUE);
            if current.is_some_and(|effect| {
                effect.amplifier >= EFFECT_AMPLIFIER
                    && (effect.duration < 0 || effect.duration > EFFECT_DISPLAY_LIMIT - 1)
            }) {
                continue;
            }
            player.living_entity.add_effect_with_source(
                Effect {
                    effect_type: &StatusEffect::MINING_FATIGUE,
                    duration: EFFECT_DURATION,
                    amplifier: EFFECT_AMPLIFIER,
                    ambient: false,
                    show_particles: true,
                    show_icon: true,
                    blend: false,
                },
                Some(self),
            );
            player.try_send_client_packet(&packet);
        }
    }
}

impl Mob for ElderGuardianEntity {
    fn get_walk_target_value(
        &self,
        level: &dyn CreatureSpawnLevel,
        pos: &pumpkin_util::math::position::BlockPos,
    ) -> f32 {
        guardian_walk_target_value(level, pos)
    }

    /// Vanilla `Guardian.checkSpawnObstruction`.
    fn check_spawn_obstruction(&self, level: &dyn CreatureSpawnLevel) -> bool {
        crate::entity::mob::is_unobstructed(self, level)
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn get_max_look_pitch_change(&self) -> f32 {
        180.0
    }

    fn before_mob_ai_step(&self) {
        guardian_ai_step(self, &self.state);
    }

    fn travel_in_water(&self, caller: &dyn EntityBase, _gravity: f64, _falling: bool) {
        guardian_travel_in_water(self, caller, &self.state);
    }

    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        let entity = &self.mob_entity.living_entity.entity;
        if entity
            .tick_count
            .load(Ordering::Relaxed)
            .wrapping_add(entity.entity_id)
            % EFFECT_INTERVAL
            == 0
        {
            self.apply_mining_fatigue();
        }

        if !self.mob_entity.has_position_target() {
            self.mob_entity
                .set_home_to(entity.block_pos.load(), HOME_RADIUS);
        }
    }

    fn pre_damage(&self, damage_type: DamageType, source: Option<&dyn EntityBase>) -> bool {
        guardian_pre_damage(self, &self.state, damage_type, source);
        true
    }
}
