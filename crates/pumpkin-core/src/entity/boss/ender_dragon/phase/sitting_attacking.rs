use super::{EnderDragonPhase, Phase, sitting_on_hurt};
use crate::entity::EntityBase;
use crate::entity::boss::ender_dragon::EnderDragonEntity;
use std::sync::atomic::{AtomicI32, Ordering};

const ROAR_DURATION: i32 = 40;

/// Vanilla `DragonSittingAttackingPhase`: roar for two seconds, then breathe fire. The roar sound
/// is played by the client.
#[derive(Default)]
pub struct SittingAttackingPhase {
    attacking_ticks: AtomicI32,
}

impl Phase for SittingAttackingPhase {
    fn get_type(&self) -> EnderDragonPhase {
        EnderDragonPhase::SittingAttacking
    }

    fn is_sitting(&self) -> bool {
        true
    }

    fn on_hurt(&self, source: Option<&dyn EntityBase>, damage: f32) -> f32 {
        sitting_on_hurt(source, damage)
    }

    fn do_server_tick(&self, dragon: &EnderDragonEntity) {
        if self.attacking_ticks.fetch_add(1, Ordering::Relaxed) >= ROAR_DURATION {
            dragon.set_phase(EnderDragonPhase::SittingFlaming);
        }
    }

    fn begin(&self, _dragon: &EnderDragonEntity) {
        self.attacking_ticks.store(0, Ordering::Relaxed);
    }
}
