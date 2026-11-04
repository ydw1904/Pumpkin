use pumpkin_util::random::gameplay::game_random;
use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, AtomicI32, AtomicU8, Ordering},
};

use pumpkin_data::BlockState;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::math::position::BlockPos;

use crate::entity::{
    Entity, EntityBase,
    ageable::{AgeableData, AgeableMob},
    ai::goal::{
        Controls, Goal, GoalReach, breed::BreedGoal, escape_danger::EscapeDangerGoal,
        follow_parent::FollowParentGoal, look_around::RandomLookAroundGoal,
        look_at_entity::LookAtEntityGoal, melee_attack::MeleeAttackGoal, revenge::RevengeGoal,
        swim::SwimGoal, tempt::TemptGoal, wander_around::WanderAroundGoal,
    },
    mob::{Mob, MobEntity},
    passive::animal::Animal,
    player::Player,
};

const TEMPT_ITEMS: &[&Item] = &[&Item::BAMBOO];

pub const FLAG_SNEEZE: u8 = 2;
pub const FLAG_ROLL: u8 = 4;
pub const FLAG_SIT: u8 = 8;
pub const FLAG_ON_BACK: u8 = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum PandaGene {
    #[default]
    Normal = 0,
    Lazy = 1,
    Worried = 2,
    Playful = 3,
    Brown = 4,
    Weak = 5,
    Aggressive = 6,
}

impl PandaGene {
    #[must_use]
    pub const fn from_id(id: u8) -> Self {
        match id {
            1 => Self::Lazy,
            2 => Self::Worried,
            3 => Self::Playful,
            4 => Self::Brown,
            5 => Self::Weak,
            6 => Self::Aggressive,
            _ => Self::Normal,
        }
    }

    #[must_use]
    pub const fn id(self) -> u8 {
        self as u8
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Lazy => "lazy",
            Self::Worried => "worried",
            Self::Playful => "playful",
            Self::Brown => "brown",
            Self::Weak => "weak",
            Self::Aggressive => "aggressive",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Self {
        match name {
            "lazy" => Self::Lazy,
            "worried" => Self::Worried,
            "playful" => Self::Playful,
            "brown" => Self::Brown,
            "weak" => Self::Weak,
            "aggressive" => Self::Aggressive,
            _ => Self::Normal,
        }
    }

    /// Vanilla `Gene.isRecessive`.
    #[must_use]
    pub const fn is_recessive(self) -> bool {
        matches!(self, Self::Brown | Self::Weak)
    }

    /// Vanilla `Gene.getVariantFromGenes`.
    #[must_use]
    pub fn variant_from_genes(main: Self, hidden: Self) -> Self {
        if main.is_recessive() && main != hidden {
            Self::Normal
        } else {
            main
        }
    }

    #[must_use]
    pub fn random_gene() -> Self {
        let rng = game_random();
        if rng.random_range(0..16) == 0 {
            Self::from_id(rng.random_range(4..=6))
        } else if rng.random_range(0..8) == 0 {
            Self::from_id(rng.random_range(1..=3))
        } else {
            Self::Normal
        }
    }
}

pub struct PandaEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    pub main_gene: AtomicU8,
    pub hidden_gene: AtomicU8,
    pub flags: AtomicU8,
    pub eat_counter: AtomicI32,
    pub sneeze_counter: AtomicI32,
    pub unhappy_counter: AtomicI32,
    /// Vanilla `gotBamboo`: fed while it had a target, so it drops the grudge.
    got_bamboo: AtomicBool,
    /// Vanilla `didBite`: a panda that isn't aggressive bites once and calms down.
    did_bite: AtomicBool,
}

impl PandaEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let gene = PandaGene::random_gene();
        let hidden = PandaGene::random_gene();
        let panda = Self {
            mob_entity,
            ageable_data: AgeableData::default(),
            main_gene: AtomicU8::new(gene.id()),
            hidden_gene: AtomicU8::new(hidden.id()),
            flags: AtomicU8::new(0),
            eat_counter: AtomicI32::new(0),
            sneeze_counter: AtomicI32::new(0),
            unhappy_counter: AtomicI32::new(0),
            got_bamboo: AtomicBool::new(false),
            did_bite: AtomicBool::new(false),
        };
        let mob_arc = Arc::new(panda);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        {
            let goal_selector = &mob_arc.mob_entity.goals_selector;

            goal_selector.add_goal(0, Box::new(SwimGoal::default()));
            goal_selector.add_goal(2, EscapeDangerGoal::new(2.0));
            goal_selector.add_goal(2, BreedGoal::new(1.0));
            goal_selector.add_goal(
                3,
                Box::new(PandaAttackGoal {
                    melee_attack: MeleeAttackGoal::new(f64::from(1.2f32), true),
                }),
            );
            goal_selector.add_goal(4, Box::new(TemptGoal::new(1.0, TEMPT_ITEMS, false)));
            goal_selector.add_goal(
                9,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 6.0),
            );
            goal_selector.add_goal(10, Box::new(RandomLookAroundGoal::default()));
            goal_selector.add_goal(13, Box::new(FollowParentGoal::new(1.25)));
            goal_selector.add_goal(14, Box::new(WanderAroundGoal::water_avoiding(1.0)));

            // Vanilla `PandaHurtByTargetGoal(this).setAlertOthers()`.
            mob_arc.mob_entity.target_selector.add_goal(
                1,
                Box::new(
                    RevengeGoal::new(true)
                        .alerting_others()
                        .alerting_only(is_aggressive_panda)
                        .continuing_while(|mob| {
                            as_panda(mob).is_none_or(|panda| {
                                !panda.got_bamboo.load(Ordering::Relaxed)
                                    && !panda.did_bite.load(Ordering::Relaxed)
                            })
                        }),
                ),
            );
        };

        mob_arc
    }

    #[must_use]
    pub fn get_main_gene(&self) -> PandaGene {
        PandaGene::from_id(self.main_gene.load(Ordering::Relaxed))
    }

    pub fn set_main_gene(&self, gene: PandaGene) {
        self.main_gene.store(gene.id(), Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::panda::MAIN_GENE_ID,
            gene.id() as i8,
        );
    }

    #[must_use]
    pub fn get_hidden_gene(&self) -> PandaGene {
        PandaGene::from_id(self.hidden_gene.load(Ordering::Relaxed))
    }

    pub fn set_hidden_gene(&self, gene: PandaGene) {
        self.hidden_gene.store(gene.id(), Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::panda::HIDDEN_GENE_ID,
            gene.id() as i8,
        );
    }

    /// Vanilla `getVariant`.
    #[must_use]
    pub fn get_variant(&self) -> PandaGene {
        PandaGene::variant_from_genes(self.get_main_gene(), self.get_hidden_gene())
    }

    #[must_use]
    pub fn is_aggressive(&self) -> bool {
        self.get_variant() == PandaGene::Aggressive
    }

    /// Vanilla `isScared`.
    #[must_use]
    pub fn is_scared(&self) -> bool {
        self.get_variant() == PandaGene::Worried && self.get_entity().world.load().is_thundering()
    }

    /// Vanilla `canPerformAction`.
    #[must_use]
    pub fn can_perform_action(&self) -> bool {
        !self.is_on_back()
            && !self.is_scared()
            && self.eat_counter.load(Ordering::Relaxed) <= 0
            && !self.is_rolling()
            && !self.is_sitting()
    }

    #[must_use]
    pub fn has_flag(&self, flag: u8) -> bool {
        (self.flags.load(Ordering::Relaxed) & flag) != 0
    }

    pub fn set_flag(&self, flag: u8, val: bool) {
        let current = self.flags.load(Ordering::Relaxed);
        let new_flags = if val { current | flag } else { current & !flag };
        self.flags.store(new_flags, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::panda::DATA_ID_FLAGS,
            new_flags as i8,
        );
    }

    #[must_use]
    pub fn is_sneezing(&self) -> bool {
        self.has_flag(FLAG_SNEEZE)
    }

    pub fn set_sneezing(&self, val: bool) {
        self.set_flag(FLAG_SNEEZE, val);
    }

    #[must_use]
    pub fn is_sitting(&self) -> bool {
        self.has_flag(FLAG_SIT)
    }

    pub fn set_sitting(&self, val: bool) {
        self.set_flag(FLAG_SIT, val);
    }

    #[must_use]
    pub fn is_on_back(&self) -> bool {
        self.has_flag(FLAG_ON_BACK)
    }

    pub fn set_on_back(&self, val: bool) {
        self.set_flag(FLAG_ON_BACK, val);
    }

    #[must_use]
    pub fn is_rolling(&self) -> bool {
        self.has_flag(FLAG_ROLL)
    }

    pub fn set_rolling(&self, val: bool) {
        self.set_flag(FLAG_ROLL, val);
    }
}

impl AgeableMob for PandaEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }

    fn default_baby_spawn_chance(&self) -> f32 {
        crate::entity::ageable::PANDA_BABY_SPAWN_CHANCE
    }
}

impl Animal for PandaEntity {
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack.item.has_tag(&tag::Item::MINECRAFT_PANDA_FOOD)
            || item_stack.item == &Item::BAMBOO
            || item_stack.item == &Item::CAKE
    }
}

impl Mob for PandaEntity {
    fn can_dispenser_equip_into_slot(
        &self,
        slot: &pumpkin_data::data_component_impl::EquipmentSlot,
    ) -> bool {
        *slot == pumpkin_data::data_component_impl::EquipmentSlot::MAIN_HAND
            && self.mob_entity.can_pick_up_loot()
    }

    // Needs pandas eating from their hand first.
    fn wants_to_pick_up(&self, _stack: &ItemStack) -> bool {
        false
    }

    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {
        self.play_entity_sound(Sound::EntityPandaStep, 0.15, 1.0);
    }

    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn as_animal(&self) -> Option<&dyn Animal> {
        Some(self)
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.write_ageable_nbt(nbt);
        nbt.put_string("MainGene", self.get_main_gene().as_str().to_string());
        nbt.put_string("HiddenGene", self.get_hidden_gene().as_str().to_string());
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.read_ageable_nbt(nbt);
        if let Some(main) = nbt.get_string("MainGene") {
            self.set_main_gene(PandaGene::from_name(main));
        }
        if let Some(hidden) = nbt.get_string("HiddenGene") {
            self.set_hidden_gene(PandaGene::from_name(hidden));
        }
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        let is_baby = entity.age.load(Ordering::Relaxed) < 0;
        if is_baby {
            entity.set_synced_data(pumpkin_data::tracked_data::panda::DATA_BABY_ID, true);
        }
        entity.set_synced_data(
            pumpkin_data::tracked_data::panda::MAIN_GENE_ID,
            self.main_gene.load(Ordering::Relaxed) as i8,
        );
        entity.set_synced_data(
            pumpkin_data::tracked_data::panda::HIDDEN_GENE_ID,
            self.hidden_gene.load(Ordering::Relaxed) as i8,
        );
        entity.set_synced_data(
            pumpkin_data::tracked_data::panda::DATA_ID_FLAGS,
            self.flags.load(Ordering::Relaxed) as i8,
        );
        entity.set_synced_data(
            pumpkin_data::tracked_data::panda::UNHAPPY_COUNTER,
            VarInt(self.unhappy_counter.load(Ordering::Relaxed)),
        );
        entity.set_synced_data(
            pumpkin_data::tracked_data::panda::SNEEZE_COUNTER,
            VarInt(self.sneeze_counter.load(Ordering::Relaxed)),
        );
        entity.set_synced_data(
            pumpkin_data::tracked_data::panda::EAT_COUNTER,
            VarInt(self.eat_counter.load(Ordering::Relaxed)),
        );
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        if self.is_food(item_stack) && self.mob_entity.get_target().is_some() {
            self.got_bamboo.store(true, Ordering::Relaxed);
        }
        self.animal_interact(player, item_stack)
    }

    /// Vanilla `Panda.doHurtTarget`.
    fn do_hurt_target(&self, target: &dyn EntityBase) -> bool {
        if !self.is_aggressive() {
            self.did_bite.store(true, Ordering::Relaxed);
        }
        let damaged = self.mob_entity.try_attack(self.get_entity(), target);
        if damaged {
            self.on_attack(target);
        }
        damaged
    }

    /// The part of vanilla `Panda.tick` that forgets the bite and the bamboo with the target.
    fn post_tick(&self) {
        if self.mob_entity.get_target().is_none() {
            self.got_bamboo.store(false, Ordering::Relaxed);
            self.did_bite.store(false, Ordering::Relaxed);
        }
    }
}

fn as_panda(mob: &dyn Mob) -> Option<&PandaEntity> {
    mob.cast_any().downcast_ref::<PandaEntity>()
}

fn is_aggressive_panda(mob: &dyn Mob) -> bool {
    as_panda(mob).is_some_and(PandaEntity::is_aggressive)
}

/// Vanilla `Panda.PandaAttackGoal`: the melee attack, only while the panda can act.
struct PandaAttackGoal {
    melee_attack: MeleeAttackGoal,
}

impl Goal for PandaAttackGoal {
    fn tick_reach(&self, mob: &dyn Mob, running: bool) -> Option<GoalReach> {
        self.melee_attack.tick_reach(mob, running)
    }

    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        as_panda(mob).is_some_and(PandaEntity::can_perform_action)
            && self.melee_attack.can_start(mob)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.melee_attack.should_continue(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.melee_attack.start(mob);
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.melee_attack.stop(mob);
    }

    fn tick(&mut self, mob: &dyn Mob) {
        self.melee_attack.tick(mob);
    }

    fn should_run_every_tick(&self) -> bool {
        self.melee_attack.should_run_every_tick()
    }

    fn controls(&self) -> Controls {
        self.melee_attack.controls()
    }
}
