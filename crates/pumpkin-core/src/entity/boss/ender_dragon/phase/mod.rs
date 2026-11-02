use crate::entity::EntityBase;
use crate::entity::boss::ender_dragon::EnderDragonEntity;
use pumpkin_data::entity::EntityType;
use pumpkin_util::math::vector3::Vector3;
use std::sync::Arc;

mod charging;
mod circling;
mod dying;
mod fly_to_portal;
mod hovering;
mod landing;
mod landing_approach;
mod sit_attacking;
mod sit_breathing;
mod strafing;
mod taking_off;

pub use charging::ChargingPhase;
pub use circling::CirclingPhase;
pub use dying::DyingPhase;
pub use fly_to_portal::FlyToPortalPhase;
pub use hovering::HoveringPhase;
pub use landing::LandingPhase;
pub use landing_approach::LandingApproachPhase;
pub use sit_attacking::SitAttackingPhase;
pub use sit_breathing::SitBreathingPhase;
pub use strafing::StrafingPhase;
pub use taking_off::TakingOffPhase;

pub trait Phase: Send + Sync {
    fn get_type(&self) -> EnderDragonPhase;
    fn tick(&self, dragon: &EnderDragonEntity);
    fn begin(&self, _dragon: &EnderDragonEntity) {}
    fn end(&self, _dragon: &EnderDragonEntity) {}
    /// Vanilla `DragonPhaseInstance.isSitting`, which decides whether hits count towards taking
    /// off again.
    fn is_sitting(&self) -> bool {
        false
    }
    fn get_fly_speed(&self) -> f32 {
        0.6
    }
    /// Vanilla `AbstractDragonPhaseInstance.getTurnSpeed`.
    fn get_turn_speed(&self, dragon: &EnderDragonEntity) -> f32 {
        let rot_speed = dragon.horizontal_speed() + 1.0;
        0.7 / rot_speed.min(40.0) / rot_speed
    }
    /// Vanilla `DragonPhaseInstance.onHurt`: the damage left after the phase reacted to the hit.
    fn on_hurt(&self, _source: Option<&dyn EntityBase>, damage: f32) -> f32 {
        damage
    }
    fn get_fly_target_location(&self) -> Option<Vector3<f64>> {
        None
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum EnderDragonPhase {
    #[default]
    Circling = 0,
    Strafing = 1,
    Charging = 2,
    FlyToPortal = 3,
    LandingApproach = 4,
    Landing = 5,
    SitAttacking = 6,
    SitBreathing = 7,
    TakingOff = 8,
    Hovering = 9,
    Dying = 10,
}

impl EnderDragonPhase {
    #[must_use]
    pub const fn is_sitting(self) -> bool {
        matches!(
            self,
            Self::LandingApproach
                | Self::Landing
                | Self::SitAttacking
                | Self::SitBreathing
                | Self::TakingOff
        )
    }

    #[must_use]
    pub const fn network_id(self) -> i32 {
        self as i32
    }
}

pub struct PhaseManager {
    phases: [Arc<dyn Phase>; 11],
}

impl Default for PhaseManager {
    fn default() -> Self {
        Self::new()
    }
}

impl PhaseManager {
    #[must_use]
    pub fn new() -> Self {
        Self {
            phases: [
                Arc::new(CirclingPhase),
                Arc::new(StrafingPhase),
                Arc::new(ChargingPhase),
                Arc::new(FlyToPortalPhase),
                Arc::new(LandingApproachPhase),
                Arc::new(LandingPhase),
                Arc::new(SitAttackingPhase),
                Arc::new(SitBreathingPhase),
                Arc::new(TakingOffPhase),
                Arc::new(HoveringPhase),
                Arc::new(DyingPhase),
            ],
        }
    }

    #[must_use]
    pub fn get_phase(&self, phase_type: EnderDragonPhase) -> Arc<dyn Phase> {
        self.phases[phase_type as usize].clone()
    }
}

/// Vanilla `AbstractDragonSittingPhase.onHurt`: a sitting dragon sets arrows and wind charges on
/// fire and takes no damage from them.
fn sitting_on_hurt(source: Option<&dyn EntityBase>, damage: f32) -> f32 {
    match source {
        Some(direct)
            if [
                &EntityType::ARROW,
                &EntityType::SPECTRAL_ARROW,
                &EntityType::TRIDENT,
                &EntityType::WIND_CHARGE,
            ]
            .iter()
            .any(|entity_type| std::ptr::eq(direct.get_entity().entity_type, *entity_type)) =>
        {
            direct.set_on_fire_for(1.0);
            0.0
        }
        _ => damage,
    }
}
