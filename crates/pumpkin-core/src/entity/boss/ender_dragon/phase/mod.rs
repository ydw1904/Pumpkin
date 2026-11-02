use crate::entity::EntityBase;
use crate::entity::boss::ender_dragon::{DragonNode, DragonPath, EnderDragonEntity};
use crate::entity::player::Player;
use pumpkin_data::entity::EntityType;
use pumpkin_util::math::vector3::Vector3;
use std::sync::Arc;

mod charge_player;
mod dying;
mod holding_pattern;
mod hover;
mod landing;
mod landing_approach;
mod sitting_attacking;
mod sitting_flaming;
mod sitting_scanning;
mod strafe_player;
mod takeoff;

pub use charge_player::ChargePlayerPhase;
pub use dying::DyingPhase;
pub use holding_pattern::HoldingPatternPhase;
pub use hover::HoverPhase;
pub use landing::LandingPhase;
pub use landing_approach::LandingApproachPhase;
pub use sitting_attacking::SittingAttackingPhase;
pub use sitting_flaming::SittingFlamingPhase;
pub use sitting_scanning::SittingScanningPhase;
pub use strafe_player::StrafePlayerPhase;
pub use takeoff::TakeoffPhase;

/// Vanilla `DragonPhaseInstance`. Each dragon owns one instance of every phase, which keeps its
/// own state between activations like vanilla's lazily created instances.
pub trait Phase: Send + Sync {
    fn get_type(&self) -> EnderDragonPhase;
    fn do_server_tick(&self, _dragon: &EnderDragonEntity) {}
    fn begin(&self, _dragon: &EnderDragonEntity) {}
    fn end(&self, _dragon: &EnderDragonEntity) {}
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
    fn on_crystal_destroyed(&self, _dragon: &EnderDragonEntity, _player: Option<&Arc<Player>>) {}
    fn get_fly_target_location(&self) -> Option<Vector3<f64>> {
        None
    }
}

/// Vanilla `EnderDragonPhase`, in registration order: the discriminant is the network and NBT id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnderDragonPhase {
    HoldingPattern = 0,
    StrafePlayer = 1,
    LandingApproach = 2,
    Landing = 3,
    Takeoff = 4,
    SittingFlaming = 5,
    SittingScanning = 6,
    SittingAttacking = 7,
    ChargingPlayer = 8,
    Dying = 9,
    Hovering = 10,
}

impl EnderDragonPhase {
    #[must_use]
    pub const fn id(self) -> i32 {
        self as i32
    }

    /// Vanilla `EnderDragonPhase.getById`: unknown ids fall back to the holding pattern.
    #[must_use]
    pub const fn from_id(id: i32) -> Self {
        match id {
            1 => Self::StrafePlayer,
            2 => Self::LandingApproach,
            3 => Self::Landing,
            4 => Self::Takeoff,
            5 => Self::SittingFlaming,
            6 => Self::SittingScanning,
            7 => Self::SittingAttacking,
            8 => Self::ChargingPlayer,
            9 => Self::Dying,
            10 => Self::Hovering,
            _ => Self::HoldingPattern,
        }
    }
}

/// Vanilla `EnderDragonPhaseManager`, minus the current phase, which the dragon keeps.
#[derive(Default)]
pub struct PhaseManager {
    pub holding_pattern: HoldingPatternPhase,
    pub strafe_player: StrafePlayerPhase,
    pub landing_approach: LandingApproachPhase,
    pub landing: LandingPhase,
    pub takeoff: TakeoffPhase,
    pub sitting_flaming: SittingFlamingPhase,
    pub sitting_scanning: SittingScanningPhase,
    pub sitting_attacking: SittingAttackingPhase,
    pub charging_player: ChargePlayerPhase,
    pub dying: DyingPhase,
    pub hovering: HoverPhase,
}

impl PhaseManager {
    #[must_use]
    pub fn get_phase(&self, phase_type: EnderDragonPhase) -> &dyn Phase {
        match phase_type {
            EnderDragonPhase::HoldingPattern => &self.holding_pattern,
            EnderDragonPhase::StrafePlayer => &self.strafe_player,
            EnderDragonPhase::LandingApproach => &self.landing_approach,
            EnderDragonPhase::Landing => &self.landing,
            EnderDragonPhase::Takeoff => &self.takeoff,
            EnderDragonPhase::SittingFlaming => &self.sitting_flaming,
            EnderDragonPhase::SittingScanning => &self.sitting_scanning,
            EnderDragonPhase::SittingAttacking => &self.sitting_attacking,
            EnderDragonPhase::ChargingPlayer => &self.charging_player,
            EnderDragonPhase::Dying => &self.dying,
            EnderDragonPhase::Hovering => &self.hovering,
        }
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

/// Vanilla `navigateToNextPathNode` of the holding pattern, strafe and landing approach phases:
/// fly to the next path node, somewhere up to 20 blocks above it.
fn navigate_to_next_path_node(
    path: &mut Option<DragonPath>,
    target_location: &mut Option<Vector3<f64>>,
) {
    if let Some(path) = path
        && !path.is_done()
    {
        let current = path.next_node_pos();
        path.advance();
        *target_location = Some(random_point_above(current));
    }
}

/// The `do { y = node.y + random * 20 } while (y < node.y)` loop vanilla repeats in every path
/// following phase.
fn random_point_above(node: DragonNode) -> Vector3<f64> {
    let y = loop {
        let y = f64::from(node.y)
            + f64::from(pumpkin_util::random::gameplay::game_random().random::<f32>() * 20.0);
        if y >= f64::from(node.y) {
            break y;
        }
    };
    Vector3::new(f64::from(node.x), y, f64::from(node.z))
}

/// Vanilla's ring choice when picking a holding pattern node: the outer ring while crystals are
/// standing, the middle ring once they're gone.
const fn wrap_node_index(index: i32, outer_ring: bool) -> usize {
    if outer_ring {
        index.rem_euclid(12) as usize
    } else {
        (((index - 12) & 7) + 12) as usize
    }
}

/// Vanilla's `distToTarget < 100.0 || distToTarget > 22500.0` check for picking a new target.
fn needs_new_target(dist_sq: f64) -> bool {
    !(100.0..=22500.0).contains(&dist_sq)
}
