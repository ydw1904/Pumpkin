use std::sync::{
    Arc, Mutex, Weak,
    atomic::{AtomicBool, AtomicI32, AtomicU8, Ordering},
};

use crossbeam::atomic::AtomicCell;

use pumpkin_data::Block;
use pumpkin_data::BlockState;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::entity::EntityType;
use pumpkin_data::entity_variant::PandaGene;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_inventory::screen_handler::InventoryPlayer;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

use crate::entity::{
    Entity, EntityBase,
    ageable::{AgeableData, AgeableMob},
    ai::behavior::fleeing::{self, Flee},
    ai::control::{Control, MoveControlTrait, move_control::MoveControl},
    ai::goal::{
        Controls, Goal, GoalReach, avoid_entity::not_spectator, breed::BreedGoal,
        escape_danger::EscapeDangerGoal, follow_parent::FollowParentGoal,
        look_around::RandomLookAroundGoal, look_at_entity::LookAtEntityGoal,
        melee_attack::MeleeAttackGoal, revenge::RevengeGoal, swim::SwimGoal, tempt::TemptGoal,
        to_goal_ticks, wander_around::WanderAroundGoal,
    },
    ai::target_match::{SearchArea, TargetMatch},
    ai::target_predicate::TargetPredicate,
    item::ItemEntity,
    mob::{
        Mob, MobEntity,
        spawn::{EntitySpawnReason, SpawnGroupData},
    },
    passive::animal::Animal,
    player::Player,
};
use crate::world::World;
use crate::world::loot::{LootContextParameters, generate_loot_from_handle};

const TEMPT_ITEMS: &[&Item] = &[&Item::BAMBOO];

pub const FLAG_SNEEZE: u8 = 2;
pub const FLAG_ROLL: u8 = 4;
pub const FLAG_SIT: u8 = 8;
pub const FLAG_ON_BACK: u8 = 16;

pub const TOTAL_ROLL_STEPS: i32 = 32;
const TOTAL_UNHAPPY_TIME: i32 = 32;
const EAT_TICK_INTERVAL: i32 = 5;
/// Vanilla `BREED_TARGETING`: the player an unhappy panda looks at.
const BREED_TARGETING_RANGE: f64 = 8.0;
const PANDA_SNEEZE_LOOT_TABLE: &str = "minecraft:gameplay/panda_sneeze";

/// Vanilla `PandaAvoidGoal`: worried pandas keep away from players and monsters.
const PANDA_FLEES: &[Flee] = &[
    Flee::new(6, TargetMatch::Type(&EntityType::PLAYER), 8.0, 2.0, 2.0)
        .matching(not_spectator)
        .when(is_worried_and_can_act),
    Flee::new(6, TargetMatch::Monster, 4.0, 2.0, 2.0)
        .matching(not_spectator)
        .when(is_worried_and_can_act),
];

const GENE_RANDOM_BOUND: i32 = 16;
const MUTATION_CHANCE: i32 = 32;
const WEAK_MAX_HEALTH: f32 = 10.0;
const LAZY_MOVEMENT_SPEED: f32 = 0.07;

/// A recessive main gene only shows when the hidden gene is the same one.
fn get_variant_from_genes(main_gene: PandaGene, hidden_gene: PandaGene) -> PandaGene {
    if main_gene.recessive() && main_gene != hidden_gene {
        PandaGene::Normal
    } else {
        main_gene
    }
}

/// `next_int(bound)` stands for vanilla's `RandomSource.nextInt`.
fn get_random_gene(mut next_int: impl FnMut(i32) -> i32) -> PandaGene {
    match next_int(GENE_RANDOM_BOUND) {
        0 => PandaGene::Lazy,
        1 => PandaGene::Worried,
        2 => PandaGene::Playful,
        4 => PandaGene::Aggressive,
        3 | 5..=8 => PandaGene::Weak,
        9 | 10 => PandaGene::Brown,
        _ => PandaGene::Normal,
    }
}

fn next_boolean(next_int: &mut impl FnMut(i32) -> i32) -> bool {
    next_int(2) != 0
}

/// Vanilla `getOneOfGenesRandomly` for a panda's (main, hidden) genes.
fn get_one_of_genes_randomly(
    genes: (PandaGene, PandaGene),
    next_int: &mut impl FnMut(i32) -> i32,
) -> PandaGene {
    if next_boolean(next_int) {
        genes.0
    } else {
        genes.1
    }
}

/// Vanilla `setGeneFromParents`: the (main, hidden) genes of a foal of two pandas.
fn genes_from_parents(
    parent1: (PandaGene, PandaGene),
    parent2: (PandaGene, PandaGene),
    mut next_int: impl FnMut(i32) -> i32,
) -> (PandaGene, PandaGene) {
    let (first, second) = if next_boolean(&mut next_int) {
        (parent1, parent2)
    } else {
        (parent2, parent1)
    };
    let mut main_gene = get_one_of_genes_randomly(first, &mut next_int);
    let mut hidden_gene = get_one_of_genes_randomly(second, &mut next_int);
    if next_int(MUTATION_CHANCE) == 0 {
        main_gene = get_random_gene(&mut next_int);
    }
    if next_int(MUTATION_CHANCE) == 0 {
        hidden_gene = get_random_gene(&mut next_int);
    }
    (main_gene, hidden_gene)
}

pub struct PandaEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    pub main_gene: AtomicI32,
    pub hidden_gene: AtomicI32,
    pub flags: AtomicU8,
    pub eat_counter: AtomicI32,
    pub sneeze_counter: AtomicI32,
    pub unhappy_counter: AtomicI32,
    /// Vanilla `gotBamboo`: fed while it had a target, so it drops the grudge.
    got_bamboo: AtomicBool,
    /// Vanilla `didBite`: a panda that isn't aggressive bites once and calms down.
    did_bite: AtomicBool,
    roll_counter: AtomicI32,
    roll_delta: AtomicCell<Vector3<f64>>,
    /// Vanilla `PandaBreedGoal` calling `lookAtPlayerGoal.setTarget`, picked up by the look goal.
    look_at_player: Mutex<Option<Arc<dyn EntityBase>>>,
}

impl PandaEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        // Vanilla only does this when not a baby, but the age is not loaded yet at this point.
        mob_entity.set_can_pick_up_loot(true);
        let panda = Self {
            mob_entity,
            ageable_data: AgeableData::default(),
            main_gene: AtomicI32::new(PandaGene::Normal.id()),
            hidden_gene: AtomicI32::new(PandaGene::Normal.id()),
            flags: AtomicU8::new(0),
            eat_counter: AtomicI32::new(0),
            sneeze_counter: AtomicI32::new(0),
            unhappy_counter: AtomicI32::new(0),
            got_bamboo: AtomicBool::new(false),
            did_bite: AtomicBool::new(false),
            roll_counter: AtomicI32::new(0),
            roll_delta: AtomicCell::new(Vector3::new(0.0, 0.0, 0.0)),
            look_at_player: Mutex::new(None),
        };
        // Vanilla `PandaMoveControl`: no walking while the panda sits, rolls, eats or lies down.
        *panda
            .mob_entity
            .move_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Box::new(PandaMoveControl(MoveControl::default()));
        let mob_arc = Arc::new(panda);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        {
            let goal_selector = &mob_arc.mob_entity.goals_selector;

            goal_selector.add_goal(0, Box::new(SwimGoal::default()));
            // Vanilla `PandaPanicGoal`.
            goal_selector.add_goal(
                2,
                EscapeDangerGoal::environmental(2.0)
                    .stopping_when(|mob| as_panda(mob).is_some_and(Self::is_sitting)),
            );
            goal_selector.add_goal(
                2,
                Box::new(PandaBreedGoal {
                    breed: BreedGoal::new(1.0),
                    unhappy_cooldown: 0,
                }),
            );
            goal_selector.add_goal(
                3,
                Box::new(PandaAttackGoal {
                    melee_attack: MeleeAttackGoal::new(f64::from(1.2f32), true),
                }),
            );
            goal_selector.add_goal(4, Box::new(TemptGoal::new(1.0, TEMPT_ITEMS, false)));
            fleeing::apply(goal_selector, PANDA_FLEES);
            goal_selector.add_goal(7, Box::new(PandaSitGoal { cooldown: 0 }));
            goal_selector.add_goal(8, Box::new(PandaLieOnBackGoal { cooldown: 0 }));
            goal_selector.add_goal(8, Box::new(PandaSneezeGoal));
            goal_selector.add_goal(
                9,
                Box::new(PandaLookAtPlayerGoal {
                    look: *LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 6.0),
                }),
            );
            goal_selector.add_goal(10, Box::new(RandomLookAroundGoal::default()));
            goal_selector.add_goal(12, Box::new(PandaRollGoal));
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
        PandaGene::by_id(self.main_gene.load(Ordering::Relaxed))
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
        PandaGene::by_id(self.hidden_gene.load(Ordering::Relaxed))
    }

    pub fn set_hidden_gene(&self, gene: PandaGene) {
        self.hidden_gene.store(gene.id(), Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::panda::HIDDEN_GENE_ID,
            gene.id() as i8,
        );
    }

    /// Vanilla `isScared`.
    #[must_use]
    pub fn is_scared(&self) -> bool {
        self.is_worried() && self.get_entity().world.load().is_thundering()
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
    pub fn get_variant(&self) -> PandaGene {
        get_variant_from_genes(self.get_main_gene(), self.get_hidden_gene())
    }

    #[must_use]
    pub fn is_lazy(&self) -> bool {
        self.get_variant() == PandaGene::Lazy
    }

    #[must_use]
    pub fn is_worried(&self) -> bool {
        self.get_variant() == PandaGene::Worried
    }

    #[must_use]
    pub fn is_playful(&self) -> bool {
        self.get_variant() == PandaGene::Playful
    }

    #[must_use]
    pub fn is_brown(&self) -> bool {
        self.get_variant() == PandaGene::Brown
    }

    #[must_use]
    pub fn is_weak(&self) -> bool {
        self.get_variant() == PandaGene::Weak
    }

    #[must_use]
    pub fn is_aggressive(&self) -> bool {
        self.get_variant() == PandaGene::Aggressive
    }

    pub fn set_attributes(&self) {
        let living = &self.mob_entity.living_entity;
        if self.is_weak() {
            living.set_max_health(WEAK_MAX_HEALTH);
        }
        if self.is_lazy() {
            living.set_attribute_base(&Attributes::MOVEMENT_SPEED, f64::from(LAZY_MOVEMENT_SPEED));
        }
    }

    pub fn set_gene_from_parents(&self, parent1: &Self, parent2: &Self) {
        let rng = self.get_random();
        let (main_gene, hidden_gene) = genes_from_parents(
            (parent1.get_main_gene(), parent1.get_hidden_gene()),
            (parent2.get_main_gene(), parent2.get_hidden_gene()),
            |bound| rng.random_range(0..bound),
        );
        self.set_main_gene(main_gene);
        self.set_hidden_gene(hidden_gene);
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

    /// Vanilla `sneeze`.
    pub fn set_sneezing(&self, val: bool) {
        self.set_flag(FLAG_SNEEZE, val);
        if !val {
            self.set_sneeze_counter(0);
        }
    }

    fn set_sneeze_counter(&self, value: i32) {
        self.sneeze_counter.store(value, Ordering::Relaxed);
        self.get_entity().set_synced_data(
            pumpkin_data::tracked_data::panda::SNEEZE_COUNTER,
            VarInt(value),
        );
    }

    fn set_unhappy_counter(&self, value: i32) {
        self.unhappy_counter.store(value, Ordering::Relaxed);
        self.get_entity().set_synced_data(
            pumpkin_data::tracked_data::panda::UNHAPPY_COUNTER,
            VarInt(value),
        );
    }

    #[must_use]
    pub fn is_eating(&self) -> bool {
        self.eat_counter.load(Ordering::Relaxed) > 0
    }

    /// Vanilla `eat`.
    pub fn set_eating(&self, val: bool) {
        self.set_eat_counter(i32::from(val));
    }

    fn set_eat_counter(&self, value: i32) {
        self.eat_counter.store(value, Ordering::Relaxed);
        self.get_entity().set_synced_data(
            pumpkin_data::tracked_data::panda::EAT_COUNTER,
            VarInt(value),
        );
    }

    fn main_hand(&self) -> ItemStack {
        self.mob_entity
            .living_entity
            .entity_equipment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&EquipmentSlot::MAIN_HAND)
    }

    /// Vanilla `tryToSit`.
    fn try_to_sit(&self) {
        let living = &self.mob_entity.living_entity;
        if !living.entity.is_in_water() {
            let input = living.movement_input.load();
            living
                .movement_input
                .store(Vector3::new(input.x, input.y, 0.0));
            self.mob_entity.steered().navigator().stop();
            self.set_sitting(true);
        }
    }

    /// Vanilla `handleEating`.
    fn handle_eating(&self) {
        let rng = self.get_random();
        let main_hand = self.main_hand();
        if !self.is_eating()
            && self.is_sitting()
            && !self.is_scared()
            && !main_hand.is_empty()
            && rng.random_range(0..80) == 1
        {
            self.set_eating(true);
        } else if main_hand.is_empty() || !self.is_sitting() {
            self.set_eating(false);
        }

        if !self.is_eating() {
            return;
        }
        let eat_counter = self.eat_counter.load(Ordering::Relaxed);
        // Of vanilla `addEatingParticles` only the sound reaches clients; they add the particles.
        if eat_counter % EAT_TICK_INTERVAL == 0 {
            let volume = 0.5f32.mul_add(rng.random_range(0..2) as f32, 0.5);
            let pitch = (rng.random::<f32>() - rng.random::<f32>()).mul_add(0.2, 1.0);
            self.play_entity_sound(Sound::EntityPandaEat, volume, pitch);
        }
        if eat_counter > 80 && rng.random_range(0..20) == 1 {
            if eat_counter > 100
                && self
                    .main_hand()
                    .item
                    .has_tag(&tag::Item::MINECRAFT_PANDA_EATS_FROM_GROUND)
            {
                self.mob_entity
                    .set_item_slot(&EquipmentSlot::MAIN_HAND, ItemStack::EMPTY.clone());
                let entity = self.get_entity();
                entity
                    .world
                    .load()
                    .emit_game_event(GameEvent::Eat.name(), entity.pos.load());
                self.set_sitting(false);
            }
            self.set_eating(false);
            return;
        }
        self.set_eat_counter(eat_counter + 1);
    }

    /// Vanilla `handleRoll`.
    fn handle_roll(&self) {
        let roll_counter = self.roll_counter.fetch_add(1, Ordering::Relaxed) + 1;
        if roll_counter > TOTAL_ROLL_STEPS {
            self.set_rolling(false);
            return;
        }
        let entity = self.get_entity();
        let movement = entity.velocity.load();
        if roll_counter == 1 {
            let angle = entity.yaw.load().to_radians();
            let multiplier = if self.is_baby() { 0.1 } else { 0.2 };
            let roll_delta = Vector3::new(
                movement.x + f64::from(-angle.sin() * multiplier),
                0.0,
                movement.z + f64::from(angle.cos() * multiplier),
            );
            self.roll_delta.store(roll_delta);
            entity
                .velocity
                .store(roll_delta + Vector3::new(0.0, 0.27, 0.0));
        } else if roll_counter != 7 && roll_counter != 15 && roll_counter != 23 {
            let roll_delta = self.roll_delta.load();
            entity
                .velocity
                .store(Vector3::new(roll_delta.x, movement.y, roll_delta.z));
        } else {
            let y = if entity.on_ground.load(Ordering::Relaxed) {
                0.27
            } else {
                movement.y
            };
            entity.velocity.store(Vector3::new(0.0, y, 0.0));
        }
    }

    /// Vanilla `afterSneeze`; clients add the sneeze particle themselves.
    fn after_sneeze(&self) {
        self.play_entity_sound(Sound::EntityPandaSneeze, 1.0, 1.0);
        let entity = self.get_entity();
        let world = entity.world.load_full();
        let area = entity.bounding_box.load().expand(10.0, 10.0, 10.0);
        for other in world.get_entities_at_box(&area) {
            let Some(panda) = other.cast_any().downcast_ref::<Self>() else {
                continue;
            };
            let panda_entity = panda.get_entity();
            if !panda.is_baby()
                && panda_entity.on_ground.load(Ordering::Relaxed)
                && !panda_entity.is_in_water()
                && panda.can_perform_action()
            {
                panda.mob_entity.living_entity.jump(panda);
            }
        }

        if !world.level_info.load().game_rules.mob_drops {
            return;
        }
        let Some(table) = world.get_loot_table(PANDA_SNEEZE_LOOT_TABLE) else {
            return;
        };
        let params = LootContextParameters {
            world: Some(&world),
            position: Some(entity.pos.load()),
            this_entity: Some(self),
            ..Default::default()
        };
        for stack in generate_loot_from_handle(&table, self.get_random().random(), &params) {
            entity.spawn_at_location(stack);
        }
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
    }

    fn get_breed_offspring(&self, partner: &dyn EntityBase) -> Arc<dyn EntityBase> {
        let entity = self.get_entity();
        let baby = Self::new(Entity::new(
            entity.world.load_full(),
            entity.pos.load(),
            &EntityType::PANDA,
        ));
        if let Some(partner) = partner.cast_any().downcast_ref::<Self>() {
            baby.set_gene_from_parents(self, partner);
        }
        baby.set_attributes();
        baby
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

    /// Vanilla `Panda.pickUpItem`: an empty-handed panda takes a whole stack of food to eat.
    fn pick_up_item(&self, item: &ItemEntity) {
        if !self.main_hand().is_empty() || !can_pick_up_and_eat(item) {
            return;
        }
        let stack = item
            .get_item_stack()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let living = &self.mob_entity.living_entity;
        if !living.pickup(item.get_entity(), u32::from(stack.item_count)) {
            return;
        }
        living.on_item_pickup(item);
        self.mob_entity
            .set_item_slot(&EquipmentSlot::MAIN_HAND, stack);
        self.mob_entity
            .set_guaranteed_drop(&EquipmentSlot::MAIN_HAND);
        item.get_entity().remove();
    }

    /// Vanilla `Panda.hurtServer`: any hurt makes it stand up.
    fn mob_hurt_server(
        &self,
        _caller: &dyn EntityBase,
        _amount: f32,
        _damage_type: pumpkin_data::damage::DamageType,
        _position: Option<Vector3<f64>>,
        _source: Option<&dyn EntityBase>,
        _cause: Option<&dyn EntityBase>,
    ) -> Option<bool> {
        self.set_sitting(false);
        None
    }

    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {
        self.play_entity_sound(Sound::EntityPandaStep, 0.15, 1.0);
    }

    fn can_be_leashed(&self) -> bool {
        false
    }

    fn get_ambient_sound(&self) -> Option<Sound> {
        Some(if self.is_aggressive() {
            Sound::EntityPandaAggressiveAmbient
        } else if self.is_worried() {
            Sound::EntityPandaWorriedAmbient
        } else {
            Sound::EntityPandaAmbient
        })
    }

    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn as_animal(&self) -> Option<&dyn Animal> {
        Some(self)
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_string("MainGene", self.get_main_gene().name().to_string());
        nbt.put_string("HiddenGene", self.get_hidden_gene().name().to_string());
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.set_main_gene(
            nbt.get_string("MainGene")
                .and_then(PandaGene::from_name)
                .unwrap_or(PandaGene::Normal),
        );
        self.set_hidden_gene(
            nbt.get_string("HiddenGene")
                .and_then(PandaGene::from_name)
                .unwrap_or(PandaGene::Normal),
        );
    }

    fn finalize_spawn(
        &self,
        _world: &Arc<World>,
        _reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        let rng = self.get_random();
        let mut next_int = |bound| rng.random_range(0..bound);
        self.set_main_gene(get_random_gene(&mut next_int));
        self.set_hidden_gene(get_random_gene(&mut next_int));
        self.set_attributes();
        self.finalize_spawn_super(group_data)
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

    /// Vanilla `Panda.mobInteract`.
    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        if self.is_scared() {
            return false;
        }
        if self.is_on_back() {
            self.set_on_back(false);
            return true;
        }
        if !self.is_food(item_stack) {
            return self.animal_interact(player, item_stack);
        }
        if self.mob_entity.get_target().is_some() {
            self.got_bamboo.store(true, Ordering::Relaxed);
        }

        let gamemode = player.gamemode.load();
        if self.can_age_up() {
            let age = self.get_age();
            item_stack.decrement_unless_creative(gamemode, 1);
            self.age_up(
                crate::entity::ageable::get_speed_up_seconds_when_feeding(-age),
                true,
            );
        } else if self.is_baby() {
            return false;
        } else if self.mob_entity.is_breeding_ready() && !self.mob_entity.is_in_love() {
            item_stack.decrement_unless_creative(gamemode, 1);
            self.set_in_love(player);
        } else {
            if self.is_sitting() || self.get_entity().is_in_water() {
                return false;
            }
            self.try_to_sit();
            self.set_eating(true);
            let current = self.main_hand();
            if !current.is_empty() && !player.has_infinite_materials() {
                self.get_entity().spawn_at_location(current);
            }
            self.mob_entity.set_item_slot(
                &EquipmentSlot::MAIN_HAND,
                ItemStack::new(1, item_stack.item),
            );
            item_stack.decrement_unless_creative(gamemode, 1);
        }
        true
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

    /// The part of vanilla `Panda.tick` after `super.tick()`.
    fn post_tick(&self) {
        let entity = self.get_entity();
        if self.is_worried() {
            if entity.world.load().is_thundering() && !entity.is_in_water() {
                self.set_sitting(true);
                self.set_eating(false);
            } else if !self.is_eating() {
                self.set_sitting(false);
            }
        }

        let target = self.mob_entity.get_target();
        if target.is_none() {
            self.got_bamboo.store(false, Ordering::Relaxed);
            self.did_bite.store(false, Ordering::Relaxed);
        }

        let unhappy_counter = self.unhappy_counter.load(Ordering::Relaxed);
        if unhappy_counter > 0 {
            if let Some(target) = &target {
                self.mob_entity.look_at(target.as_ref(), 90.0, 90.0);
            }
            if unhappy_counter == 29 || unhappy_counter == 14 {
                self.play_entity_sound(Sound::EntityPandaCantBreed, 1.0, 1.0);
            }
            self.set_unhappy_counter(unhappy_counter - 1);
        }

        if self.is_sneezing() {
            let sneeze_counter = self.sneeze_counter.load(Ordering::Relaxed) + 1;
            self.set_sneeze_counter(sneeze_counter);
            if sneeze_counter > 20 {
                self.set_sneezing(false);
                self.after_sneeze();
            } else if sneeze_counter == 1 {
                self.play_entity_sound(Sound::EntityPandaPreSneeze, 1.0, 1.0);
            }
        }

        if self.is_rolling() {
            self.handle_roll();
        } else {
            self.roll_counter.store(0, Ordering::Relaxed);
        }

        if self.is_sitting() {
            entity.set_pitch(0.0);
        }

        self.handle_eating();
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

fn is_worried_and_can_act(mob: &dyn Mob) -> bool {
    as_panda(mob).is_some_and(|panda| panda.is_worried() && panda.can_perform_action())
}

/// Vanilla `Panda.canPickUpAndEat`.
fn can_pick_up_and_eat(item: &ItemEntity) -> bool {
    item.get_item_stack()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .item
        .has_tag(&tag::Item::MINECRAFT_PANDA_EATS_FROM_GROUND)
        && item.get_entity().is_alive()
        && item.get_pickup_delay() == 0
}

/// The items within `range` of the panda's box that it would pick up and eat.
fn edible_items_nearby(panda: &PandaEntity, range: f64) -> Vec<Arc<dyn EntityBase>> {
    let entity = panda.get_entity();
    let area = entity.bounding_box.load().expand(range, range, range);
    let mut items = entity.world.load().get_entities_at_box(&area);
    items.retain(|candidate| candidate.get_item_entity().is_some_and(can_pick_up_and_eat));
    items
}

/// `canContinueToUse` of vanilla `PandaSitGoal` and `PandaLieOnBackGoal`.
fn keeps_sitting_or_lying(panda: &PandaEntity) -> bool {
    let rng = panda.get_random();
    !panda.get_entity().is_in_water()
        && (panda.is_lazy() || rng.random_range(0..to_goal_ticks(600)) != 1)
        && rng.random_range(0..to_goal_ticks(2000)) != 1
}

fn tick_count(panda: &PandaEntity) -> i32 {
    panda.get_entity().tick_count.load(Ordering::Relaxed)
}

/// Vanilla `Panda.PandaMoveControl`.
struct PandaMoveControl(MoveControl);

impl Control for PandaMoveControl {}

impl MoveControlTrait for PandaMoveControl {
    fn tick(&mut self, mob: &dyn Mob) {
        if as_panda(mob).is_some_and(PandaEntity::can_perform_action) {
            self.0.tick(mob);
        }
    }

    fn set_wanted_position(&mut self, x: f64, y: f64, z: f64, speed_modifier: f64) {
        self.0.set_wanted_position(x, y, z, speed_modifier);
    }

    fn strafe(&mut self, forward: f32, right: f32) {
        self.0.strafe(forward, right);
    }

    fn has_wanted(&self) -> bool {
        self.0.has_wanted()
    }
}

/// Vanilla `Panda.PandaBreedGoal`: no bamboo nearby makes the panda unhappy instead.
struct PandaBreedGoal {
    breed: Box<BreedGoal>,
    unhappy_cooldown: i32,
}

impl PandaBreedGoal {
    /// Vanilla `canFindBamboo`: bamboo within 8 blocks around and 2 above the panda's feet.
    fn can_find_bamboo(panda: &PandaEntity) -> bool {
        let entity = panda.get_entity();
        let panda_pos = entity.block_pos.load();
        let world = entity.world.load();
        for y_off in 0..3 {
            for r in 0..8 {
                let mut x = 0;
                while x <= r {
                    let mut z = if x < r && x > -r { r } else { 0 };
                    while z <= r {
                        let pos = panda_pos.offset(Vector3::new(x, y_off, z));
                        if world.get_block(&pos) == &Block::BAMBOO {
                            return true;
                        }
                        z = if z > 0 { -z } else { 1 - z };
                    }
                    x = if x > 0 { -x } else { 1 - x };
                }
            }
        }
        false
    }

    /// Vanilla `level.getNearestPlayer(BREED_TARGETING, panda)`.
    fn nearest_player(mob: &dyn Mob) -> Option<Arc<dyn EntityBase>> {
        let entity = mob.get_entity();
        let world = entity.world.load();
        let conditions =
            TargetPredicate::create_non_attackable().set_base_max_distance(BREED_TARGETING_RANGE);
        let area = SearchArea::from_feet(entity, BREED_TARGETING_RANGE, BREED_TARGETING_RANGE)
            .players_anywhere();
        TargetMatch::Type(&EntityType::PLAYER).find_nearest(&world, &area, |candidate| {
            conditions.test(&world, Some(mob), candidate)
        })
    }
}

impl Goal for PandaBreedGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(panda) = as_panda(mob) else {
            return false;
        };
        if !self.breed.can_start(mob) || panda.unhappy_counter.load(Ordering::Relaxed) != 0 {
            return false;
        }
        if Self::can_find_bamboo(panda) {
            return true;
        }
        let tick = tick_count(panda);
        if self.unhappy_cooldown <= tick {
            panda.set_unhappy_counter(TOTAL_UNHAPPY_TIME);
            self.unhappy_cooldown = tick + 600;
            let player = Self::nearest_player(mob);
            *panda
                .look_at_player
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = player;
        }
        false
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.breed.should_continue(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.breed.start(mob);
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.breed.stop(mob);
    }

    fn tick(&mut self, mob: &dyn Mob) {
        self.breed.tick(mob);
    }

    fn should_run_every_tick(&self) -> bool {
        self.breed.should_run_every_tick()
    }

    fn controls(&self) -> Controls {
        self.breed.controls()
    }
}

/// Vanilla `Panda.PandaLookAtPlayerGoal`. Its target lives on the panda, where the breed goal
/// sets it.
struct PandaLookAtPlayerGoal {
    look: LookAtEntityGoal,
}

impl PandaLookAtPlayerGoal {
    fn with_target<R>(
        &mut self,
        mob: &dyn Mob,
        callback: impl FnOnce(&mut LookAtEntityGoal, &PandaEntity) -> R,
    ) -> Option<R> {
        let panda = as_panda(mob)?;
        self.look.target.clone_from(
            &panda
                .look_at_player
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        let result = callback(&mut self.look, panda);
        panda
            .look_at_player
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone_from(&self.look.target);
        Some(result)
    }
}

impl Goal for PandaLookAtPlayerGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        self.with_target(mob, |look, panda| {
            if mob.get_random().random::<f32>() >= look.chance {
                return false;
            }
            if look.target.is_none() {
                look.target = look.find_target(mob);
            }
            panda.can_perform_action() && look.target.is_some()
        })
        .unwrap_or(false)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.with_target(mob, |look, _| {
            look.target.is_some() && look.should_continue(mob)
        })
        .unwrap_or(false)
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.look.start(mob);
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.with_target(mob, |look, _| look.stop(mob));
    }

    fn tick(&mut self, mob: &dyn Mob) {
        self.with_target(mob, |look, _| {
            if look.target.is_some() {
                look.tick(mob);
            }
        });
    }

    fn should_run_every_tick(&self) -> bool {
        self.look.should_run_every_tick()
    }

    fn controls(&self) -> Controls {
        self.look.controls()
    }
}

/// Vanilla `Panda.PandaSitGoal`: sit down and eat what it holds, or walk to food on the ground.
struct PandaSitGoal {
    cooldown: i32,
}

impl Goal for PandaSitGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(panda) = as_panda(mob) else {
            return false;
        };
        if self.cooldown > tick_count(panda)
            || panda.is_baby()
            || panda.get_entity().is_in_water()
            || !panda.can_perform_action()
            || panda.unhappy_counter.load(Ordering::Relaxed) > 0
        {
            return false;
        }
        !panda.main_hand().is_empty() || !edible_items_nearby(panda, 6.0).is_empty()
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        as_panda(mob).is_some_and(keeps_sitting_or_lying)
    }

    fn tick(&mut self, mob: &dyn Mob) {
        if let Some(panda) = as_panda(mob)
            && !panda.is_sitting()
            && !panda.main_hand().is_empty()
        {
            panda.try_to_sit();
        }
    }

    fn start(&mut self, mob: &dyn Mob) {
        let Some(panda) = as_panda(mob) else {
            return;
        };
        if panda.main_hand().is_empty() {
            if let Some(item) = edible_items_nearby(panda, 8.0).first() {
                let destination = item.get_entity().block_pos.load().to_f64();
                let steered = panda.mob_entity.steered();
                let living = &steered.mob_entity().living_entity;
                let mut navigator = steered.navigator();
                let path = navigator.create_path(living, destination, 1);
                navigator.move_to_path(path, f64::from(1.2f32), living);
            }
        } else {
            panda.try_to_sit();
        }
        self.cooldown = 0;
    }

    fn stop(&mut self, mob: &dyn Mob) {
        let Some(panda) = as_panda(mob) else {
            return;
        };
        let held = panda.main_hand();
        if !held.is_empty() {
            panda.get_entity().spawn_at_location(held);
            panda
                .mob_entity
                .set_item_slot(&EquipmentSlot::MAIN_HAND, ItemStack::EMPTY.clone());
            let rng = panda.get_random();
            let wait_seconds = if panda.is_lazy() {
                rng.random_range(0..50) + 10
            } else {
                rng.random_range(0..150) + 10
            };
            self.cooldown = tick_count(panda) + wait_seconds * 20;
        }
        panda.set_sitting(false);
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}

/// Vanilla `Panda.PandaLieOnBackGoal`: lazy pandas lie down now and then.
struct PandaLieOnBackGoal {
    cooldown: i32,
}

impl Goal for PandaLieOnBackGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        as_panda(mob).is_some_and(|panda| {
            self.cooldown < tick_count(panda)
                && panda.is_lazy()
                && panda.can_perform_action()
                && panda.get_random().random_range(0..to_goal_ticks(400)) == 1
        })
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        as_panda(mob).is_some_and(keeps_sitting_or_lying)
    }

    fn start(&mut self, mob: &dyn Mob) {
        if let Some(panda) = as_panda(mob) {
            panda.set_on_back(true);
        }
        self.cooldown = 0;
    }

    fn stop(&mut self, mob: &dyn Mob) {
        if let Some(panda) = as_panda(mob) {
            panda.set_on_back(false);
            self.cooldown = tick_count(panda) + 200;
        }
    }
}

/// Vanilla `Panda.PandaSneezeGoal`: babies sneeze, weak ones more often.
struct PandaSneezeGoal;

impl Goal for PandaSneezeGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        as_panda(mob).is_some_and(|panda| {
            let rng = panda.get_random();
            panda.is_baby()
                && panda.can_perform_action()
                && ((panda.is_weak() && rng.random_range(0..to_goal_ticks(500)) == 1)
                    || rng.random_range(0..to_goal_ticks(6000)) == 1)
        })
    }

    fn should_continue(&mut self, _mob: &dyn Mob) -> bool {
        false
    }

    fn start(&mut self, mob: &dyn Mob) {
        if let Some(panda) = as_panda(mob) {
            panda.set_sneezing(true);
        }
    }
}

/// Vanilla `Panda.PandaRollGoal`: babies and playful pandas roll over, always at a ledge.
struct PandaRollGoal;

impl Goal for PandaRollGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(panda) = as_panda(mob) else {
            return false;
        };
        let entity = panda.get_entity();
        if !(panda.is_baby() || panda.is_playful())
            || !entity.on_ground.load(Ordering::Relaxed)
            || !panda.can_perform_action()
        {
            return false;
        }
        let angle = entity.yaw.load().to_radians();
        let step = |dir: f32| {
            if dir.abs() > 0.5 {
                dir.signum() as i32
            } else {
                0
            }
        };
        let below_ahead =
            entity
                .block_pos
                .load()
                .offset(Vector3::new(step(-angle.sin()), -1, step(angle.cos())));
        if entity.world.load().get_block_state(&below_ahead).is_air() {
            return true;
        }
        let rng = panda.get_random();
        (panda.is_playful() && rng.random_range(0..to_goal_ticks(60)) == 1)
            || rng.random_range(0..to_goal_ticks(500)) == 1
    }

    fn should_continue(&mut self, _mob: &dyn Mob) -> bool {
        false
    }

    fn start(&mut self, mob: &dyn Mob) {
        if let Some(panda) = as_panda(mob) {
            panda.set_rolling(true);
        }
    }

    fn can_stop(&self) -> bool {
        false
    }

    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK | Controls::JUMP
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::ai::test_support::world;
    use pumpkin_util::math::vector3::Vector3;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn panda(world: &Arc<World>) -> Arc<PandaEntity> {
        PandaEntity::new(Entity::new(
            world.clone(),
            Vector3::new(0.5, 64.0, 0.5),
            &EntityType::PANDA,
        ))
    }

    fn set_genes(panda: &PandaEntity, main_gene: PandaGene, hidden_gene: PandaGene) {
        panda.set_main_gene(main_gene);
        panda.set_hidden_gene(hidden_gene);
    }

    /// Runs `genes_from_parents` against scripted `(bound, result)` random calls.
    fn inherit(
        parent1: (PandaGene, PandaGene),
        parent2: (PandaGene, PandaGene),
        calls: &[(i32, i32)],
    ) -> (PandaGene, PandaGene) {
        let mut script = calls.iter();
        let genes = genes_from_parents(parent1, parent2, |bound| {
            let (expected_bound, result) = script.next().copied().unwrap_or((-1, -1));
            assert_eq!(bound, expected_bound);
            result
        });
        assert!(script.next().is_none(), "unused scripted random calls");
        genes
    }

    #[test]
    fn random_gene_follows_vanilla_roll_table() {
        let expected = [
            PandaGene::Lazy,
            PandaGene::Worried,
            PandaGene::Playful,
            PandaGene::Weak,
            PandaGene::Aggressive,
            PandaGene::Weak,
            PandaGene::Weak,
            PandaGene::Weak,
            PandaGene::Weak,
            PandaGene::Brown,
            PandaGene::Brown,
            PandaGene::Normal,
            PandaGene::Normal,
            PandaGene::Normal,
            PandaGene::Normal,
            PandaGene::Normal,
        ];
        for (roll, gene) in (0i32..).zip(expected) {
            let mut asked_bound = 0;
            let rolled = get_random_gene(|bound| {
                asked_bound = bound;
                roll
            });
            assert_eq!(rolled, gene, "roll {roll}");
            assert_eq!(asked_bound, 16);
        }
    }

    #[test]
    fn recessive_main_gene_shows_only_with_matching_hidden_gene() {
        let variant = get_variant_from_genes;
        assert_eq!(
            variant(PandaGene::Brown, PandaGene::Brown),
            PandaGene::Brown
        );
        assert_eq!(variant(PandaGene::Weak, PandaGene::Weak), PandaGene::Weak);
        assert_eq!(
            variant(PandaGene::Brown, PandaGene::Weak),
            PandaGene::Normal
        );
        assert_eq!(
            variant(PandaGene::Weak, PandaGene::Normal),
            PandaGene::Normal
        );
        // A dominant main gene ignores the hidden one, and a recessive hidden gene never shows.
        assert_eq!(variant(PandaGene::Lazy, PandaGene::Brown), PandaGene::Lazy);
        assert_eq!(
            variant(PandaGene::Normal, PandaGene::Weak),
            PandaGene::Normal
        );
    }

    #[test]
    fn foal_takes_one_random_gene_from_each_parent() {
        let parent1 = (PandaGene::Lazy, PandaGene::Worried);
        let parent2 = (PandaGene::Playful, PandaGene::Aggressive);
        // First boolean true: main from parent 1, hidden from parent 2.
        let genes = inherit(
            parent1,
            parent2,
            &[(2, 1), (2, 1), (2, 0), (32, 5), (32, 7)],
        );
        assert_eq!(genes, (PandaGene::Lazy, PandaGene::Aggressive));
        // First boolean false: main from parent 2, hidden from parent 1.
        let genes = inherit(
            parent1,
            parent2,
            &[(2, 0), (2, 0), (2, 1), (32, 5), (32, 7)],
        );
        assert_eq!(genes, (PandaGene::Aggressive, PandaGene::Lazy));
    }

    #[test]
    fn foal_genes_mutate_independently_with_one_in_thirty_two() {
        let parent = (PandaGene::Normal, PandaGene::Normal);
        let genes = inherit(
            parent,
            parent,
            &[(2, 1), (2, 1), (2, 1), (32, 0), (16, 3), (32, 4)],
        );
        assert_eq!(genes, (PandaGene::Weak, PandaGene::Normal));
        let genes = inherit(
            parent,
            parent,
            &[(2, 1), (2, 1), (2, 1), (32, 9), (32, 0), (16, 9)],
        );
        assert_eq!(genes, (PandaGene::Normal, PandaGene::Brown));
    }

    #[tokio::test]
    async fn weak_variant_halves_health_and_lazy_variant_slows_down() -> TestResult {
        let folder = tempfile::tempdir()?;
        let world = world(&folder);
        let default_speed = panda(&world)
            .mob_entity
            .living_entity
            .get_attribute_value(&Attributes::MOVEMENT_SPEED);

        let weak = panda(&world);
        set_genes(&weak, PandaGene::Weak, PandaGene::Weak);
        weak.set_attributes();
        let living = &weak.mob_entity.living_entity;
        assert!((living.get_max_health() - 10.0).abs() < f32::EPSILON);
        assert!((living.health.load() - 10.0).abs() < f32::EPSILON);
        assert!(
            (living.get_attribute_value(&Attributes::MOVEMENT_SPEED) - default_speed).abs() < 1e-9
        );

        let lazy = panda(&world);
        set_genes(&lazy, PandaGene::Lazy, PandaGene::Normal);
        lazy.set_attributes();
        let living = &lazy.mob_entity.living_entity;
        assert!((living.get_attribute_value(&Attributes::MOVEMENT_SPEED) - 0.07).abs() < 1e-6);
        assert!((living.get_max_health() - 20.0).abs() < f32::EPSILON);

        // A hidden recessive gene does not make the panda weak.
        let carrier = panda(&world);
        set_genes(&carrier, PandaGene::Weak, PandaGene::Normal);
        carrier.set_attributes();
        assert!((carrier.mob_entity.living_entity.get_max_health() - 20.0).abs() < f32::EPSILON);
        Ok(())
    }

    #[tokio::test]
    async fn genes_round_trip_through_nbt_and_default_to_normal() -> TestResult {
        let folder = tempfile::tempdir()?;
        let world = world(&folder);
        let source = panda(&world);
        set_genes(&source, PandaGene::Brown, PandaGene::Weak);
        source.set_age(-100);
        let mut nbt = NbtCompound::new();
        source.write_custom_nbt(&mut nbt);
        assert_eq!(nbt.get_string("MainGene"), Some("brown"));
        assert_eq!(nbt.get_string("HiddenGene"), Some("weak"));

        let loaded = panda(&world);
        set_genes(&loaded, PandaGene::Lazy, PandaGene::Lazy);
        loaded.read_custom_nbt(&nbt);
        assert_eq!(loaded.get_main_gene(), PandaGene::Brown);
        assert_eq!(loaded.get_hidden_gene(), PandaGene::Weak);
        assert_eq!(loaded.get_age(), -100);

        set_genes(&loaded, PandaGene::Weak, PandaGene::Brown);
        loaded.read_custom_nbt(&NbtCompound::new());
        assert_eq!(loaded.get_main_gene(), PandaGene::Normal);
        assert_eq!(loaded.get_hidden_gene(), PandaGene::Normal);
        Ok(())
    }

    #[tokio::test]
    async fn offspring_inherits_genes_and_gets_attributes() -> TestResult {
        let folder = tempfile::tempdir()?;
        let world = world(&folder);
        let parent = panda(&world);
        set_genes(&parent, PandaGene::Weak, PandaGene::Weak);
        let mate = panda(&world);
        set_genes(&mate, PandaGene::Weak, PandaGene::Weak);

        // Each gene mutates with 1/32, so nearly all foals of two weak pandas are weak.
        let mut weak_foals = 0;
        for _ in 0..40 {
            let baby = parent.get_breed_offspring(mate.as_ref());
            let foal = baby
                .cast_any()
                .downcast_ref::<PandaEntity>()
                .ok_or("offspring is not a panda")?;
            if foal.is_weak() {
                weak_foals += 1;
                assert!(
                    (foal.mob_entity.living_entity.get_max_health() - 10.0).abs() < f32::EPSILON
                );
            }
        }
        assert!(weak_foals >= 20, "{weak_foals} weak foals out of 40");
        Ok(())
    }

    #[tokio::test]
    async fn panda_rules_follow_vanilla() -> TestResult {
        let folder = tempfile::tempdir()?;
        let world = world(&folder);
        let panda = panda(&world);

        assert!(panda.is_food(&ItemStack::new(1, &Item::BAMBOO)));
        assert!(!panda.is_food(&ItemStack::new(1, &Item::CAKE)));
        assert!(!panda.can_be_leashed());
        assert!(panda.mob_entity.can_pick_up_loot());

        let ambient = |main_gene, hidden_gene| {
            set_genes(&panda, main_gene, hidden_gene);
            panda.get_ambient_sound()
        };
        assert_eq!(
            ambient(PandaGene::Aggressive, PandaGene::Normal),
            Some(Sound::EntityPandaAggressiveAmbient)
        );
        assert_eq!(
            ambient(PandaGene::Worried, PandaGene::Normal),
            Some(Sound::EntityPandaWorriedAmbient)
        );
        assert_eq!(
            ambient(PandaGene::Brown, PandaGene::Weak),
            Some(Sound::EntityPandaAmbient)
        );
        Ok(())
    }
}
