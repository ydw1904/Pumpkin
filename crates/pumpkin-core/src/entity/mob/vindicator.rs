use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::enchantment_provider::EnchantmentProvider;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::difficulty::Difficulty;
use pumpkin_util::text::TextComponent;

use crate::entity::ai::behavior::fleeing::{self, Flee};
use crate::entity::ai::behavior::hunting::{self, Hunt};
use crate::entity::ai::goal::active_target::ActiveTargetGoal;
use crate::entity::ai::goal::break_door::BreakDoorGoal;
use crate::entity::ai::goal::{Controls, Goal, GoalReach, to_goal_ticks};
use crate::entity::ai::target_match::TargetMatch;
use crate::entity::living::LivingEntity;
use crate::entity::mob::spawn::{EntitySpawnReason, SpawnGroupData};
use crate::entity::{
    Entity, EntityBase,
    ai::goal::{
        look_at_entity::LookAtEntityGoal, melee_attack::MeleeAttackGoal, swim::SwimGoal,
        wander_around::WanderAroundGoal,
    },
    mob::{
        Mob, MobEntity,
        patrol::{LongDistancePatrolGoal, PatrolData, PatrollingMonster},
        raider::{
            self, HoldGroundAttackGoal, ObtainRaidLeaderBannerGoal, PathfindToRaidGoal, Raider,
            RaiderCelebrationGoal, RaiderData, RaiderMoveThroughVillageGoal, RaiderOpenDoorGoal,
        },
    },
};
use crate::world::World;
use crate::world::raid::Raid;

const VINDICATOR_FLEES: &[Flee] = &[raider::flee_creaking(1, 1.0, 1.2)];

const VINDICATOR_HUNTS: &[Hunt] = &[
    Hunt::new(2, TargetMatch::Type(&EntityType::PLAYER), true),
    Hunt::new(3, TargetMatch::VILLAGER_LIKE, true),
    Hunt::new(3, TargetMatch::Type(&EntityType::IRON_GOLEM), true),
];

/// Vanilla `Vindicator.TAG_JOHNNY`, both the save key and the name that sets it.
const TAG_JOHNNY: &str = "Johnny";
/// Vanilla passes 6 to `BreakDoorGoal`, which keeps its 240 tick minimum.
const DOOR_BREAK_TIME: i32 = 6;

pub struct VindicatorEntity {
    pub mob_entity: MobEntity,
    pub raider_data: RaiderData,
    is_johnny: Arc<AtomicBool>,
    /// The custom name last seen, to spot vanilla `setCustomName` calls.
    seen_name: Mutex<Arc<Option<TextComponent>>>,
}

impl VindicatorEntity {
    #[must_use]
    pub fn new(entity: Entity) -> Arc<Self> {
        let seen_name = Mutex::new(entity.custom_name.load_full());
        let mob_entity = MobEntity::new(entity);
        let vindicator = Self {
            mob_entity,
            raider_data: RaiderData::default(),
            is_johnny: Arc::new(AtomicBool::new(false)),
            seen_name,
        };
        let mob_arc = Arc::new(vindicator);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        {
            let goal_selector = &mob_arc.mob_entity.goals_selector;

            // `PatrollingMonster` and `Raider` register theirs first.
            goal_selector.add_goal(4, Box::new(LongDistancePatrolGoal::new(0.7, 0.595)));
            goal_selector.add_goal(1, Box::new(ObtainRaidLeaderBannerGoal));
            goal_selector.add_goal(3, Box::new(PathfindToRaidGoal::default()));
            goal_selector.add_goal(4, Box::new(RaiderMoveThroughVillageGoal::new(1.05)));
            goal_selector.add_goal(5, Box::new(RaiderCelebrationGoal));

            goal_selector.add_goal(0, Box::new(SwimGoal::default()));
            fleeing::apply(goal_selector, VINDICATOR_FLEES);
            goal_selector.add_goal(2, Box::new(VindicatorBreakDoorGoal::new()));
            goal_selector.add_goal(3, Box::new(RaiderOpenDoorGoal::default()));
            goal_selector.add_goal(4, Box::new(HoldGroundAttackGoal::new(10.0)));
            goal_selector.add_goal(5, Box::new(MeleeAttackGoal::new(1.0, false)));

            let target_selector = &mob_arc.mob_entity.target_selector;
            target_selector.add_goal(1, raider::revenge_goal());
            hunting::apply(target_selector, &mob_arc.mob_entity, VINDICATOR_HUNTS);
            target_selector.add_goal(
                4,
                Box::new(VindicatorJohnnyAttackGoal::new(
                    &mob_arc.mob_entity,
                    mob_arc.is_johnny.clone(),
                )),
            );

            goal_selector.add_goal(8, Box::new(WanderAroundGoal::new(0.6)));
            goal_selector.add_goal(
                9,
                Box::new(LookAtEntityGoal::new(
                    mob_weak,
                    &EntityType::PLAYER,
                    3.0,
                    1.0,
                    false,
                )),
            );
            goal_selector.add_goal(10, LookAtEntityGoal::mobs(8.0));
        };

        mob_arc
    }

    /// Vanilla `Vindicator.setCustomName`: naming it Johnny makes it one for good.
    fn notice_custom_name(&self) {
        let name = self.get_entity().custom_name.load_full();
        let mut seen = self
            .seen_name
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if Arc::ptr_eq(&seen, &name) {
            return;
        }
        if name
            .as_ref()
            .as_ref()
            .is_some_and(|name| name.clone().get_text() == TAG_JOHNNY)
        {
            self.is_johnny.store(true, Ordering::Relaxed);
        }
        *seen = name;
    }
}

impl Mob for VindicatorEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn as_patrolling_monster(&self) -> Option<&dyn PatrollingMonster> {
        Some(self)
    }

    fn as_raider(&self) -> Option<&dyn Raider> {
        Some(self)
    }

    fn mob_tick(&self, _caller: &dyn EntityBase) {
        self.notice_custom_name();
    }

    /// Vanilla `Vindicator.customServerAiStep`: doors open for it only inside a raid.
    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        let mob_entity = &self.mob_entity;
        if mob_entity.is_no_ai() {
            return;
        }
        let world = self.get_entity().world.load();
        let raided = world
            .raids
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_raid_at(&self.get_entity().block_pos.load())
            .is_some();
        let steered = mob_entity.steered();
        let mut navigator = steered.navigator();
        if navigator.can_navigate_ground() {
            navigator.set_can_open_doors(raided);
        }
    }

    fn finalize_spawn(
        &self,
        _world: &Arc<World>,
        _reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        let group_data = self.finalize_spawn_super(group_data);
        self.mob_entity
            .steered()
            .navigator()
            .set_can_open_doors(true);
        group_data
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.write_raider_nbt(nbt);
        self.notice_custom_name();
        if self.is_johnny.load(Ordering::Relaxed) {
            nbt.put_bool(TAG_JOHNNY, true);
        }
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.read_raider_nbt(nbt);
        // Vanilla reads the custom name first, so a loaded name alone doesn't count.
        *self
            .seen_name
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            self.get_entity().custom_name.load_full();
        self.is_johnny
            .store(nbt.get_bool(TAG_JOHNNY).unwrap_or(false), Ordering::Relaxed);
    }
}

impl PatrollingMonster for VindicatorEntity {
    fn get_patrol_data(&self) -> &PatrolData {
        &self.raider_data.patrol_data
    }
}

impl Raider for VindicatorEntity {
    fn get_raider_data(&self) -> &RaiderData {
        &self.raider_data
    }

    fn get_celebrate_sound(&self) -> Sound {
        Sound::EntityVindicatorCelebrate
    }

    /// Vanilla `Vindicator.applyRaidBuffs`: an iron axe, Sharpness at higher omen levels.
    fn apply_raid_buffs(&self, raid: &Raid, wave: i32, _is_captain: bool) {
        let mut axe = ItemStack::new(1, &Item::IRON_AXE);
        if self.get_random().random::<f32>() <= raid.get_enchant_odds() {
            let provider = if wave > Raid::get_num_groups(Difficulty::Normal) {
                &EnchantmentProvider::RAID_VINDICATOR_POST_WAVE_5
            } else {
                &EnchantmentProvider::RAID_VINDICATOR
            };
            provider.apply_to_stack(&mut axe, 0.0);
        }
        self.mob_entity
            .set_item_slot(&EquipmentSlot::MAIN_HAND, axe);
    }
}

fn has_active_raid(mob: &dyn Mob) -> bool {
    mob.as_raider().is_some_and(Raider::has_active_raid)
}

/// Vanilla `Vindicator.VindicatorBreakDoorGoal`: breaks doors on normal and hard, during a raid.
struct VindicatorBreakDoorGoal {
    break_door: BreakDoorGoal,
}

impl VindicatorBreakDoorGoal {
    fn new() -> Self {
        Self {
            break_door: BreakDoorGoal::with_door_break_time(
                DOOR_BREAK_TIME,
                Arc::new(|difficulty| matches!(difficulty, Difficulty::Normal | Difficulty::Hard)),
            ),
        }
    }
}

impl Goal for VindicatorBreakDoorGoal {
    fn tick_reach(&self, mob: &dyn Mob, running: bool) -> Option<GoalReach> {
        self.break_door.tick_reach(mob, running)
    }

    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        has_active_raid(mob)
            && mob.get_random().random_range(0..to_goal_ticks(10)) == 0
            && self.break_door.can_start(mob)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        has_active_raid(mob) && self.break_door.should_continue(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.break_door.start(mob);
        mob.get_mob_entity()
            .living_entity
            .no_action_time
            .store(0, Ordering::Relaxed);
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.break_door.stop(mob);
    }

    fn tick(&mut self, mob: &dyn Mob) {
        self.break_door.tick(mob);
    }

    fn should_run_every_tick(&self) -> bool {
        self.break_door.should_run_every_tick()
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}

/// Vanilla `Vindicator.VindicatorJohnnyAttackGoal`: a Johnny goes for anything attackable.
struct VindicatorJohnnyAttackGoal {
    target_goal: Box<ActiveTargetGoal>,
    is_johnny: Arc<AtomicBool>,
}

impl VindicatorJohnnyAttackGoal {
    fn new(mob: &MobEntity, is_johnny: Arc<AtomicBool>) -> Self {
        Self {
            target_goal: Box::new(ActiveTargetGoal::new(
                mob,
                TargetMatch::Any,
                0,
                true,
                true,
                Some(|target: &LivingEntity, _: &World| target.attackable()),
            )),
            is_johnny,
        }
    }
}

impl Goal for VindicatorJohnnyAttackGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        self.is_johnny.load(Ordering::Relaxed) && self.target_goal.can_start(mob)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.target_goal.should_continue(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.target_goal.start(mob);
        mob.get_mob_entity()
            .living_entity
            .no_action_time
            .store(0, Ordering::Relaxed);
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.target_goal.stop(mob);
    }

    fn controls(&self) -> Controls {
        self.target_goal.controls()
    }
}
