use pumpkin_util::random::gameplay::game_random;
use std::sync::Arc;

use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_util::Hand;
use pumpkin_util::math::rotate_if_necessary;
use pumpkin_util::math::vector3::Vector3;

use crate::entity::ai::goal::{Controls, Goal};
use crate::entity::ai::pathfinder::NavigatorGoal;
use crate::entity::mob::Mob;
use crate::entity::projectile::arrow::{ArrowEntity, ArrowPickup};
use crate::entity::{Entity, EntityBase};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CrossbowState {
    Uncharged,
    Charging,
    Charged,
    ReadyToAttack,
}

/// Vanilla `BackUpIfTooClose`: strafes back from a visible target that is too close.
#[derive(Clone, Copy)]
struct BackUp {
    distance_sq: f64,
    strafe_speed: f32,
}

/// Ranged crossbow attack used by Pillagers, Piglins, and other mobs.
/// Mirrors vanilla `RangedCrossbowAttackGoal`, or the crossbow fight of a brain mob.
pub struct RangedCrossbowAttackGoal {
    goal_control: Controls,
    speed: f64,
    squared_range: f64,
    state: CrossbowState,
    see_time: i32,
    attack_delay: i32,
    update_path_delay: i32,
    charge_ticks: i32,
    /// Set for the brain fight, see [`Self::brain`].
    back_up: Option<BackUp>,
    path_target: Option<Vector3<f64>>,
}

impl RangedCrossbowAttackGoal {
    /// Vanilla crossbow charge duration (ticks).
    const CHARGE_DURATION: i32 = 25;
    /// Vanilla arrow speed for crossbow shots.
    const ARROW_SPEED: f64 = 1.6;
    /// Vanilla `SetWalkTargetFromAttackTargetIfTargetOutOfReach.PROJECTILE_ATTACK_RANGE_BUFFER`.
    const PROJECTILE_ATTACK_RANGE_BUFFER: f64 = 1.0;

    #[must_use]
    pub fn new(speed: f64, range: f32) -> Self {
        Self {
            goal_control: Controls::MOVE | Controls::LOOK,
            speed,
            squared_range: f64::from(range * range),
            state: CrossbowState::Uncharged,
            see_time: 0,
            attack_delay: 0,
            update_path_delay: 0,
            charge_ticks: 0,
            back_up: None,
            path_target: None,
        }
    }

    /// The crossbow part of a brain mob's FIGHT activity, like `PiglinAi.initFightActivity`:
    /// `BackUpIfTooClose(back_up_distance, back_up_speed)`, then
    /// `SetWalkTargetFromAttackTargetIfTargetOutOfReach(speed)`, then `CrossbowAttack`, which
    /// charges and shoots while the target is visible and within `range`.
    #[must_use]
    pub fn brain(speed: f64, range: f32, back_up_distance: f64, back_up_speed: f32) -> Self {
        let mut goal = Self::new(speed, range);
        goal.back_up = Some(BackUp {
            distance_sq: back_up_distance * back_up_distance,
            strafe_speed: back_up_speed,
        });
        goal
    }

    #[must_use]
    pub fn is_holding_crossbow(mob: &dyn Mob) -> bool {
        let equipment_guard = mob
            .get_mob_entity()
            .living_entity
            .entity_equipment
            .try_lock();
        equipment_guard.is_ok_and(|equipment| {
            equipment.get(&EquipmentSlot::MAIN_HAND).item.id == Item::CROSSBOW.id
                || equipment.get(&EquipmentSlot::OFF_HAND).item.id == Item::CROSSBOW.id
        })
    }

    fn shoot(mob: &dyn Mob, target: &Arc<dyn EntityBase>) {
        let entity = mob.get_entity();
        let world = entity.world.load();
        let world_full = entity.world.load_full();

        let mob_pos = entity.pos.load();
        let target_entity = target.get_entity();
        let target_pos = target_entity.pos.load();

        let equipment_guard = mob
            .get_mob_entity()
            .living_entity
            .entity_equipment
            .try_lock();
        let weapon_opt = equipment_guard
            .as_ref()
            .ok()
            .map(|eq| eq.get(&EquipmentSlot::MAIN_HAND));

        let arrow_entity = Entity::new(world.clone(), mob_pos, &EntityType::ARROW);
        let projectile = ItemStack::new(1, &Item::ARROW);
        let arrow = if let Some(ref weapon) = weapon_opt {
            ArrowEntity::new_shot_with_weapon(
                arrow_entity,
                entity,
                &projectile,
                weapon,
                ArrowPickup::Allowed,
            )
        } else {
            ArrowEntity::new_shot(arrow_entity, entity, &projectile, ArrowPickup::Allowed)
        };

        if let Some(ref item) = weapon_opt {
            arrow.set_pierce_level(
                crate::enchantment::EnchantmentHelper::process_projectile_piercing(item, 0),
            );
        }
        arrow.apply_on_projectile_spawned(&projectile);
        // Vanilla `CrossbowItem.createProjectile`.
        arrow.set_sound_event(Sound::ItemCrossbowHit);
        if entity.is_on_fire() {
            arrow.set_flame(true);
        }

        let dx = target_pos.x - mob_pos.x;
        let dy = (target_pos.y + f64::from(target_entity.entity_dimension.load().height) / 3.0)
            - arrow.entity.pos.load().y;
        let dz = target_pos.z - mob_pos.z;
        let horizontal_distance = dx.hypot(dz);

        let difficulty = world.level_info.load().difficulty as i32;
        let divergence = f64::from(14 - difficulty * 4);

        arrow.set_velocity(
            dx,
            horizontal_distance.mul_add(0.2, dy),
            dz,
            Self::ARROW_SPEED,
            divergence,
        );

        world.play_sound(Sound::ItemCrossbowShoot, SoundCategory::Hostile, &mob_pos);

        let arrow: Arc<dyn EntityBase> = Arc::new(arrow);
        let entity_id = entity.entity_id;
        if let Some(server) = world_full.server.upgrade() {
            let mut event =
                crate::plugin::api::events::entity::entity_shoot_bow::EntityShootBowEvent::new(
                    entity_id,
                    "minecraft:crossbow".to_string(),
                    1.0,
                );
            server.plugin_manager.fire_blocking(&server, &mut event);
            if event.cancelled {
                return;
            }
        }
        world_full.spawn_entity(arrow);

        if let Some(crossbow_mob) = mob.as_crossbow_attack_mob() {
            crossbow_mob.on_crossbow_attack_performed();
        }
    }

    fn look_at(mob: &dyn Mob, target: &Arc<dyn EntityBase>) {
        mob.get_mob_entity()
            .look_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .look_at_entity_with_range(target, 30.0, 30.0);
    }

    fn stop_charging(mob: &dyn Mob) {
        mob.get_mob_entity().living_entity.clear_active_hand();
        if let Some(crossbow_mob) = mob.as_crossbow_attack_mob() {
            crossbow_mob.set_charging_crossbow(false);
        }
    }

    /// `RangedCrossbowAttackGoal.tick`'s movement; returns whether the mob still needs to move.
    fn tick_goal_movement(
        &mut self,
        mob: &dyn Mob,
        target: &Arc<dyn EntityBase>,
        distance_sq: f64,
        has_line_of_sight: bool,
    ) -> bool {
        if has_line_of_sight {
            self.see_time += 1;
        } else {
            self.see_time = 0;
        }

        let needs_to_move =
            (distance_sq > self.squared_range || self.see_time < 5) && self.attack_delay == 0;

        if needs_to_move {
            self.update_path_delay -= 1;
            if self.update_path_delay <= 0 {
                let move_speed = if self.state == CrossbowState::Uncharged {
                    self.speed
                } else {
                    self.speed * 0.5
                };
                mob.get_mob_entity()
                    .steered()
                    .navigator()
                    .set_progress(NavigatorGoal {
                        current_progress: mob.get_entity().pos.load(),
                        destination: target.get_entity().pos.load(),
                        speed: move_speed,
                        reach_range: 1,
                    });
                self.update_path_delay = 20 + game_random().random_range(0..20);
            }
        } else {
            self.update_path_delay = 0;
            mob.get_mob_entity().steered().navigator().stop();
        }
        needs_to_move
    }

    /// `BackUpIfTooClose`, then `SetWalkTargetFromAttackTargetIfTargetOutOfReach`.
    fn tick_brain_movement(
        &mut self,
        mob: &dyn Mob,
        target: &Arc<dyn EntityBase>,
        distance_sq: f64,
        visible: bool,
        back_up: BackUp,
    ) {
        let mob_entity = mob.get_mob_entity();
        // It only backs off while it has no walk target.
        if visible && distance_sq < back_up.distance_sq && mob.is_navigator_idle() {
            mob_entity
                .steered()
                .move_control()
                .strafe(-back_up.strafe_speed, 0.0);
            let entity = mob.get_entity();
            entity.set_rotation(
                rotate_if_necessary(entity.yaw.load(), entity.head_yaw.load(), 0.0),
                entity.pitch.load(),
            );
        }
        // `BehaviorUtils.isWithinAttackRange` for a projectile weapon.
        let walk_range = self.squared_range.sqrt() - Self::PROJECTILE_ATTACK_RANGE_BUFFER;
        if visible && distance_sq < walk_range * walk_range {
            self.path_target = None;
            mob_entity.steered().navigator().stop();
            return;
        }
        // `MoveToTargetSink` paths again once the target moved away from the last path's end.
        let target_pos = target.get_entity().pos.load();
        let moved = self
            .path_target
            .is_none_or(|last| last.squared_distance_to_vec(&target_pos) > 1.0);
        if moved || mob.is_navigator_idle() {
            mob_entity
                .steered()
                .navigator()
                .set_progress(NavigatorGoal {
                    current_progress: mob.get_entity().pos.load(),
                    destination: target_pos,
                    speed: self.speed,
                    reach_range: 0,
                });
            self.path_target = Some(target_pos);
        }
    }

    /// The `CrossbowState` steps; `may_charge` and `may_shoot` gate the uncharged and ready ones.
    fn tick_crossbow(
        &mut self,
        mob: &dyn Mob,
        target: &Arc<dyn EntityBase>,
        may_charge: bool,
        may_shoot: bool,
    ) {
        let mob_pos = mob.get_entity().pos.load();
        match self.state {
            CrossbowState::Uncharged => {
                if may_charge {
                    let stack = mob
                        .get_mob_entity()
                        .living_entity
                        .entity_equipment
                        .try_lock()
                        .map_or_else(
                            |_| ItemStack::EMPTY.clone(),
                            |eq| eq.get(&EquipmentSlot::MAIN_HAND),
                        );
                    mob.get_mob_entity().living_entity.set_active_hand(
                        Hand::Right,
                        stack,
                        i32::MAX,
                    );
                    self.state = CrossbowState::Charging;
                    self.charge_ticks = 0;
                    if let Some(crossbow_mob) = mob.as_crossbow_attack_mob() {
                        crossbow_mob.set_charging_crossbow(true);
                    }
                    mob.get_entity().world.load().play_sound(
                        Sound::ItemCrossbowLoadingStart,
                        SoundCategory::Hostile,
                        &mob_pos,
                    );
                }
            }
            CrossbowState::Charging => {
                self.charge_ticks += 1;
                if self.charge_ticks == 10 {
                    mob.get_entity().world.load().play_sound(
                        Sound::ItemCrossbowLoadingMiddle,
                        SoundCategory::Hostile,
                        &mob_pos,
                    );
                }
                if self.charge_ticks >= Self::CHARGE_DURATION {
                    Self::stop_charging(mob);
                    self.state = CrossbowState::Charged;
                    self.attack_delay = 20 + game_random().random_range(0..20);
                    mob.get_entity().world.load().play_sound(
                        Sound::ItemCrossbowLoadingEnd,
                        SoundCategory::Hostile,
                        &mob_pos,
                    );
                }
            }
            CrossbowState::Charged => {
                self.attack_delay -= 1;
                if self.attack_delay <= 0 {
                    self.state = CrossbowState::ReadyToAttack;
                }
            }
            CrossbowState::ReadyToAttack => {
                if may_shoot {
                    Self::shoot(mob, target);
                    self.state = CrossbowState::Uncharged;
                }
            }
        }
    }
}

impl Goal for RangedCrossbowAttackGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let target = mob.get_mob_entity().get_target().clone();
        let Some(target) = target else {
            return false;
        };
        if !target.get_entity().is_alive() {
            return false;
        }
        Self::is_holding_crossbow(mob)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        let target = mob.get_mob_entity().get_target().clone();
        let Some(target) = target else {
            return false;
        };
        target.get_entity().is_alive() && Self::is_holding_crossbow(mob)
    }

    fn start(&mut self, _mob: &dyn Mob) {
        self.state = CrossbowState::Uncharged;
        self.see_time = 0;
        self.attack_delay = 0;
        self.update_path_delay = 0;
        self.charge_ticks = 0;
        self.path_target = None;
    }

    fn stop(&mut self, mob: &dyn Mob) {
        Self::stop_charging(mob);
        self.state = CrossbowState::Uncharged;
        mob.get_mob_entity().steered().navigator().stop();
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let target = mob.get_mob_entity().get_target().clone();
        let Some(target) = target else {
            return;
        };

        let distance_sq = mob
            .get_entity()
            .pos
            .load()
            .squared_distance_to_vec(&target.get_entity().pos.load());
        let has_line_of_sight = mob.has_line_of_sight(target.get_entity());

        let Some(back_up) = self.back_up else {
            let needs_to_move =
                self.tick_goal_movement(mob, &target, distance_sq, has_line_of_sight);
            Self::look_at(mob, &target);
            self.tick_crossbow(mob, &target, !needs_to_move, has_line_of_sight);
            return;
        };

        self.tick_brain_movement(mob, &target, distance_sq, has_line_of_sight, back_up);
        Self::look_at(mob, &target);
        if has_line_of_sight && distance_sq < self.squared_range {
            self.tick_crossbow(mob, &target, true, true);
        } else if self.state == CrossbowState::Charging {
            // `CrossbowAttack.stop` lets go of the half-charged crossbow.
            Self::stop_charging(mob);
            self.state = CrossbowState::Uncharged;
        }
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        self.goal_control
    }
}
