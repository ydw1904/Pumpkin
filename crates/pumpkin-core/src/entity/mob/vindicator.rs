use std::sync::{Arc, Weak};

use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::enchantment_provider::EnchantmentProvider;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::difficulty::Difficulty;

use crate::entity::ai::behavior::fleeing::{self, Flee};
use crate::entity::ai::behavior::hunting::{self, Hunt};
use crate::entity::ai::target_match::TargetMatch;
use crate::entity::{
    Entity,
    ai::goal::{
        look_around::RandomLookAroundGoal, look_at_entity::LookAtEntityGoal,
        melee_attack::MeleeAttackGoal, open_door::OpenDoorGoal, swim::SwimGoal,
        wander_around::WanderAroundGoal,
    },
    mob::{
        Mob, MobEntity,
        patrol::{LongDistancePatrolGoal, PatrolData, PatrollingMonster},
        raider::{
            self, HoldGroundAttackGoal, ObtainRaidLeaderBannerGoal, PathfindToRaidGoal, Raider,
            RaiderCelebrationGoal, RaiderData, RaiderMoveThroughVillageGoal,
        },
    },
};
use crate::world::raid::Raid;

const VINDICATOR_FLEES: &[Flee] = &[raider::flee_creaking(1, 1.0, 1.2)];

const VINDICATOR_HUNTS: &[Hunt] = &[
    Hunt::new(2, TargetMatch::Type(&EntityType::PLAYER), true),
    Hunt::new(3, TargetMatch::VILLAGER_LIKE, true),
    Hunt::new(3, TargetMatch::Type(&EntityType::IRON_GOLEM), true),
];

pub struct VindicatorEntity {
    pub mob_entity: MobEntity,
    pub raider_data: RaiderData,
}

impl VindicatorEntity {
    #[must_use]
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let vindicator = Self {
            mob_entity,
            raider_data: RaiderData::default(),
        };
        let mob_arc = Arc::new(vindicator);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        {
            let goal_selector = &mob_arc.mob_entity.goals_selector;

            goal_selector.add_goal(0, Box::new(SwimGoal::default()));
            goal_selector.add_goal(1, Box::new(OpenDoorGoal::new(false)));
            goal_selector.add_goal(1, Box::new(ObtainRaidLeaderBannerGoal));
            fleeing::apply(goal_selector, VINDICATOR_FLEES);
            goal_selector.add_goal(2, Box::new(HoldGroundAttackGoal::new(10.0)));
            goal_selector.add_goal(3, Box::new(MeleeAttackGoal::new(1.0, true)));
            goal_selector.add_goal(4, Box::new(LongDistancePatrolGoal::new(0.7, 0.595)));
            goal_selector.add_goal(4, Box::new(RaiderMoveThroughVillageGoal::new(1.05)));
            goal_selector.add_goal(4, Box::new(PathfindToRaidGoal::default()));
            goal_selector.add_goal(5, Box::new(RaiderCelebrationGoal));
            goal_selector.add_goal(5, Box::new(WanderAroundGoal::new(1.0)));
            goal_selector.add_goal(
                6,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 8.0),
            );
            goal_selector.add_goal(7, Box::new(RandomLookAroundGoal::default()));

            let target_selector = &mob_arc.mob_entity.target_selector;
            target_selector.add_goal(1, raider::revenge_goal());
            hunting::apply(target_selector, &mob_arc.mob_entity, VINDICATOR_HUNTS);
            // TODO: VindicatorJohnnyAttackGoal at 4.
        };

        mob_arc
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

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.write_raider_nbt(nbt);
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.read_raider_nbt(nbt);
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
