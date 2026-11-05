use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use pumpkin_data::Block;
use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::EntityType;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::world::BlockFlags;

use crate::entity::ai::behavior::hunting::{self, Hunt};
use crate::entity::ai::target_match::TargetMatch;
use crate::entity::{
    Entity, EntityBase,
    ai::goal::{
        look_around::RandomLookAroundGoal, look_at_entity::LookAtEntityGoal,
        ranged_attack::RangedAttackGoal, wander_around::WanderAroundGoal,
    },
    mob::{Mob, MobEntity, RangedAttackMob},
    player::Player,
    projectile::snowball::SnowballEntity,
    shearable::{Shearable, drop_from_shearing_loot_table},
};
use crate::world::game_event::GameEventContext;

const SNOW_GOLEM_HUNTS: &[Hunt] = &[Hunt::new(1, TargetMatch::Enemy, true)];

const SNOW_GOLEM_SHEAR_LOOT_TABLE: &str = "minecraft:shearing/snow_golem";

/// Vanilla `AbstractGolem.getAmbientSoundInterval`.
const AMBIENT_SOUND_INTERVAL: i32 = 120;

pub struct SnowGolemEntity {
    pub mob_entity: MobEntity,
    pub has_pumpkin: AtomicBool,
}

impl SnowGolemEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        // Vanilla `SnowGolem.defineSynchedData`.
        entity.synched_data.define(
            pumpkin_data::tracked_data::snow_golem::DATA_PUMPKIN_ID,
            16i8,
        );
        let mob_entity = MobEntity::new(entity);
        let snow_golem = Self {
            mob_entity,
            has_pumpkin: AtomicBool::new(true),
        };
        let mob_arc = Arc::new(snow_golem);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };
        let ranged_weak: Weak<dyn RangedAttackMob> = {
            let ranged_arc: Arc<dyn RangedAttackMob> = mob_arc.clone();
            Arc::downgrade(&ranged_arc)
        };

        {
            let goal_selector = &mob_arc.mob_entity.goals_selector;
            let target_selector = &mob_arc.mob_entity.target_selector;

            goal_selector.add_goal(
                1,
                Box::new(RangedAttackGoal::new(ranged_weak, 1.25, 20, 10.0)),
            );
            goal_selector.add_goal(
                2,
                Box::new(WanderAroundGoal::water_avoiding_with_probability(
                    1.0,
                    1.000_000_1E-5,
                )),
            );
            goal_selector.add_goal(
                3,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 6.0),
            );
            goal_selector.add_goal(4, Box::new(RandomLookAroundGoal::default()));

            hunting::apply(target_selector, &mob_arc.mob_entity, SNOW_GOLEM_HUNTS);
        };

        mob_arc
    }

    #[must_use]
    pub fn has_pumpkin(&self) -> bool {
        self.has_pumpkin.load(Ordering::Relaxed)
    }

    pub fn set_has_pumpkin(&self, has_pumpkin: bool) {
        self.has_pumpkin.store(has_pumpkin, Ordering::Relaxed);
        let entity = self.get_entity();
        let flags = if has_pumpkin { 16u8 } else { 0u8 };
        entity.set_synced_data(
            pumpkin_data::tracked_data::snow_golem::DATA_PUMPKIN_ID,
            flags as i8,
        );
    }

    pub fn throw_snowball(&self, target: &Arc<dyn EntityBase>) {
        let entity = self.get_entity();
        let world = entity.world.load_full();

        let snowball_entity = Entity::new(world.clone(), entity.pos.load(), &EntityType::SNOWBALL);
        let snowball = SnowballEntity::new_shot(snowball_entity, entity);

        let mob_pos = entity.pos.load();
        let target_entity = target.get_entity();
        let target_pos = target_entity.pos.load();
        let target_height = f64::from(target_entity.entity_dimension.load().height);

        let dx = target_pos.x - mob_pos.x;
        let dy = (target_pos.y + target_height / 3.0) - snowball.get_entity().pos.load().y;
        let dz = target_pos.z - mob_pos.z;
        let horizontal_distance = dx.hypot(dz);
        let yo = horizontal_distance * 0.2;

        snowball.thrown.set_velocity(dx, dy + yo, dz, 1.6, 12.0);

        if !entity.silent.load(Ordering::Relaxed) {
            world.play_sound(Sound::EntitySnowballThrow, SoundCategory::Neutral, &mob_pos);
        }

        let snowball_arc: Arc<dyn EntityBase> = Arc::new(snowball);
        world.spawn_entity(snowball_arc);
    }
}

impl Mob for SnowGolemEntity {
    fn get_ambient_sound_interval(&self) -> i32 {
        AMBIENT_SOUND_INTERVAL
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_bool("Pumpkin", self.has_pumpkin());
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        if let Some(pumpkin) = nbt.get_bool("Pumpkin") {
            self.set_has_pumpkin(pumpkin);
        }
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn as_shearable(&self) -> Option<&dyn Shearable> {
        Some(self)
    }

    fn is_sensitive_to_water(&self) -> bool {
        true
    }

    // Vanilla `SnowGolem.aiStep`: melt where it is too hot, then leave a trail of snow.
    fn post_tick(&self) {
        let entity = &self.mob_entity.living_entity.entity;
        if entity.is_removed() {
            return;
        }
        let world = entity.world.load_full();
        let pos = entity.pos.load();
        if world.snow_golem_melts(&BlockPos::containing_vec(pos)) {
            self.damage(self, 1.0, DamageType::ON_FIRE);
        }
        if !world.level_info.load().game_rules.mob_griefing {
            return;
        }

        let snow = Block::SNOW.default_state.id;
        for i in 0..4 {
            let x_offset = f64::from((i % 2 * 2 - 1) as f32 * 0.25);
            let z_offset = f64::from((i / 2 % 2 * 2 - 1) as f32 * 0.25);
            let snow_pos = BlockPos::containing(pos.x + x_offset, pos.y, pos.z + z_offset);
            if world.get_block_state(&snow_pos).is_air()
                && crate::block::blocks::snow::can_place_at(world.as_ref(), &snow_pos)
            {
                world.set_block_state(&snow_pos, snow, BlockFlags::NOTIFY_ALL);
                world.emit_game_event_with_context(
                    GameEvent::BlockPlace.name(),
                    snow_pos.to_centered_f64(),
                    &GameEventContext {
                        source_entity: Some(self),
                    },
                );
            }
        }
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        let flags = if self.has_pumpkin() { 16u8 } else { 0u8 };
        entity.set_synced_data(
            pumpkin_data::tracked_data::snow_golem::DATA_PUMPKIN_ID,
            flags as i8,
        );
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        if item_stack.get_item() == &Item::SHEARS && self.ready_for_shearing() {
            let entity = self.get_entity();
            let world = entity.world.load();
            if let Some(server) = world.server.upgrade() {
                let mut event = crate::plugin::api::events::player::player_shear_entity::PlayerShearEntityEvent {
                    player: player.clone(),
                    entity_id: entity.entity_id,
                    hand: 0,
                    cancelled: false,
                };
                server.plugin_manager.fire_blocking(&server, &mut event);
                if event.cancelled {
                    return false;
                }
            }
            self.shear(SoundCategory::Players, item_stack);
            world.emit_game_event_with_context(
                GameEvent::Shear.name(),
                entity.pos.load(),
                &GameEventContext {
                    source_entity: Some(player.as_ref()),
                },
            );
            let _ = player.damage_item_stack(item_stack, 1);
            return true;
        }
        false
    }
}

impl Shearable for SnowGolemEntity {
    fn shear(&self, sound_category: SoundCategory, tool: &ItemStack) {
        let entity = self.get_entity();
        entity.world.load().play_sound(
            Sound::EntitySnowGolemShear,
            sound_category,
            &entity.pos.load(),
        );
        self.set_has_pumpkin(false);
        drop_from_shearing_loot_table(
            self,
            SNOW_GOLEM_SHEAR_LOOT_TABLE,
            tool,
            entity.get_eye_height(),
        );
    }

    fn ready_for_shearing(&self) -> bool {
        self.has_pumpkin()
    }
}

impl RangedAttackMob for SnowGolemEntity {
    fn perform_ranged_attack(&self, target: &Arc<dyn EntityBase>, _power: f32) {
        self.throw_snowball(target);
    }
}
