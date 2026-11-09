use std::sync::{Mutex, PoisonError};

use super::BrainState;
use crate::entity::mob::Mob;

type PendingBrainUpdate<M> = Box<dyn FnOnce(&M, &mut BrainState) + Send>;

/// Brain changes from outside the brain tick that found the brain busy, such as a thorns hit
/// from the mob's own melee attack. The mob runs them right after that brain tick.
pub struct PendingBrainUpdates<M>(Mutex<Vec<PendingBrainUpdate<M>>>);

impl<M> Default for PendingBrainUpdates<M> {
    fn default() -> Self {
        Self(Mutex::new(Vec::new()))
    }
}

impl<M: Mob> PendingBrainUpdates<M> {
    /// Runs `update` on `mob`'s brain now, or right after the brain tick that holds it.
    pub fn update(&self, mob: &M, update: impl FnOnce(&M, &mut BrainState) + Send + 'static) {
        self.update_then(mob, update, |_, _| {});
    }

    /// [`Self::update`] that also runs `after_now` when the update could run right away.
    pub fn update_then(
        &self,
        mob: &M,
        update: impl FnOnce(&M, &mut BrainState) + Send + 'static,
        after_now: impl FnOnce(&M, &mut BrainState),
    ) {
        let mut update = Some(update);
        let ran = mob
            .get_mob_entity()
            .try_with_brain(mob, |brain| {
                if let Some(update) = update.take() {
                    update(mob, brain);
                }
                after_now(mob, brain);
            })
            .is_some();
        if !ran && let Some(update) = update {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(Box::new(update));
        }
    }

    /// Runs the queued updates, including any they queue themselves.
    pub fn run(&self, mob: &M, brain: &mut BrainState) {
        loop {
            let pending =
                std::mem::take(&mut *self.0.lock().unwrap_or_else(PoisonError::into_inner));
            if pending.is_empty() {
                return;
            }
            for update in pending {
                update(mob, brain);
            }
        }
    }
}
