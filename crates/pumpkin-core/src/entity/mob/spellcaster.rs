//! Vanilla `SpellcasterIllager`: the casting state and the two goals every spell runs through.
//! The evoker still keeps its own copy in `evoker.rs`.

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, AtomicU8, Ordering};

use pumpkin_data::sound::Sound;
use pumpkin_data::tracked_data::TrackedData;
use pumpkin_nbt::compound::NbtCompound;

use crate::entity::ai::goal::{Controls, Goal};
use crate::entity::mob::Mob;
use crate::entity::mob::evoker::IllagerSpell;

/// Vanilla `SpellcasterUseSpellGoal.getCastWarmupTime`.
const CAST_WARMUP_TIME: i32 = 20;

pub struct SpellcasterState {
    spell_casting_tick_count: AtomicI32,
    current_spell: AtomicU8,
    spell_data: TrackedData,
    casting_sound: Sound,
}

impl SpellcasterState {
    #[must_use]
    pub const fn new(spell_data: TrackedData, casting_sound: Sound) -> Self {
        Self {
            spell_casting_tick_count: AtomicI32::new(0),
            current_spell: AtomicU8::new(IllagerSpell::None as u8),
            spell_data,
            casting_sound,
        }
    }

    pub fn is_casting_spell(&self) -> bool {
        self.spell_casting_tick_count.load(Ordering::Relaxed) > 0
    }

    pub fn set_is_casting_spell(&self, mob: &dyn Mob, spell: IllagerSpell) {
        self.current_spell.store(spell as u8, Ordering::Relaxed);
        mob.get_entity()
            .set_synced_data(self.spell_data, spell as u8 as i8);
    }

    pub fn current_spell(&self) -> IllagerSpell {
        IllagerSpell::from_u8(self.current_spell.load(Ordering::Relaxed))
    }

    /// The tail of vanilla `SpellcasterIllager.customServerAiStep`.
    pub fn tick(&self) {
        let ticks = self.spell_casting_tick_count.load(Ordering::Relaxed);
        if ticks > 0 {
            self.spell_casting_tick_count
                .store(ticks - 1, Ordering::Relaxed);
        }
    }

    pub fn write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_int(
            "SpellTicks",
            self.spell_casting_tick_count.load(Ordering::Relaxed),
        );
    }

    pub fn read_nbt(&self, nbt: &NbtCompound) {
        self.spell_casting_tick_count
            .store(nbt.get_int("SpellTicks").unwrap_or(0), Ordering::Relaxed);
    }
}

/// Vanilla `SpellcasterCastingSpellGoal`: stands still and watches the target while casting.
pub struct SpellcasterCastingSpellGoal {
    state: Arc<SpellcasterState>,
}

impl SpellcasterCastingSpellGoal {
    #[must_use]
    pub const fn new(state: Arc<SpellcasterState>) -> Self {
        Self { state }
    }
}

impl Goal for SpellcasterCastingSpellGoal {
    fn can_start(&mut self, _mob: &dyn Mob) -> bool {
        self.state.is_casting_spell()
    }

    fn start(&mut self, mob: &dyn Mob) {
        mob.get_mob_entity().steered().navigator().stop();
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.state.set_is_casting_spell(mob, IllagerSpell::None);
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let mob_entity = mob.get_mob_entity();
        if let Some(target) = mob_entity.get_target() {
            mob_entity
                .look_control
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .look_at_entity_with_range(
                    &target,
                    mob.get_max_head_rotation(),
                    mob.get_max_look_pitch_change(),
                );
        }
    }

    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK
    }
}

/// One spell for [`SpellcasterUseSpellGoal`], the abstract methods of vanilla's goal.
pub trait Spell: Send + Sync {
    /// Checked after the shared `canUse` conditions pass.
    fn can_use(&mut self, _mob: &dyn Mob) -> bool {
        true
    }

    fn on_start(&mut self, _mob: &dyn Mob) {}

    fn casting_time(&self) -> i32;

    fn casting_interval(&self) -> i32;

    fn prepare_sound(&self) -> Option<Sound>;

    fn spell(&self) -> IllagerSpell;

    fn perform_spell_casting(&mut self, mob: &dyn Mob);
}

/// Vanilla `SpellcasterUseSpellGoal`.
pub struct SpellcasterUseSpellGoal<S: Spell> {
    state: Arc<SpellcasterState>,
    spell: S,
    attack_warmup_delay: i32,
    next_attack_tick_count: i32,
}

impl<S: Spell> SpellcasterUseSpellGoal<S> {
    #[must_use]
    pub const fn new(state: Arc<SpellcasterState>, spell: S) -> Self {
        Self {
            state,
            spell,
            attack_warmup_delay: 0,
            next_attack_tick_count: 0,
        }
    }
}

impl<S: Spell> Goal for SpellcasterUseSpellGoal<S> {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(target) = mob.get_mob_entity().get_target() else {
            return false;
        };
        if !target.get_entity().is_alive()
            || self.state.is_casting_spell()
            || mob.get_entity().tick_count.load(Ordering::Relaxed) < self.next_attack_tick_count
        {
            return false;
        }
        self.spell.can_use(mob)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        mob.get_mob_entity()
            .get_target()
            .is_some_and(|target| target.get_entity().is_alive())
            && self.attack_warmup_delay > 0
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.attack_warmup_delay = self.get_tick_count(CAST_WARMUP_TIME);
        self.state
            .spell_casting_tick_count
            .store(self.spell.casting_time(), Ordering::Relaxed);
        self.next_attack_tick_count = mob
            .get_entity()
            .tick_count
            .load(Ordering::Relaxed)
            .wrapping_add(self.spell.casting_interval());
        if let Some(sound) = self.spell.prepare_sound() {
            mob.play_entity_sound(sound, 1.0, 1.0);
        }
        self.state.set_is_casting_spell(mob, self.spell.spell());
        self.spell.on_start(mob);
    }

    fn tick(&mut self, mob: &dyn Mob) {
        self.attack_warmup_delay -= 1;
        if self.attack_warmup_delay == 0 {
            self.spell.perform_spell_casting(mob);
            mob.play_entity_sound(self.state.casting_sound, 1.0, 1.0);
        }
    }
}
