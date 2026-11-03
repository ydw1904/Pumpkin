use std::any::Any;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::entity::combat;
use crate::entity::living::LivingEntity;
use crate::entity::player::Player;
use crate::entity::{Entity, EntityBase};
use crate::item::{ItemBehaviour, ItemMetadata};
use crate::server::Server;
use crate::world::World;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::damage::DamageType;
use pumpkin_data::data_component_impl::{
    AttackRangeImpl, AttributeModifiersImpl, EnchantmentsImpl, EquipmentSlot, KineticConditionImpl,
    KineticWeaponImpl, Operation, PiercingWeaponImpl, WeaponImpl,
};
use pumpkin_data::entity::{EntityStatus, EntityType};
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::SoundCategory;
use pumpkin_data::tag::Taggable;
use pumpkin_data::{Enchantment, tag};
use pumpkin_protocol::bedrock::server::actor_event::ActorEventID;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::{GameMode, Hand};

pub struct SpearItem;

impl ItemMetadata for SpearItem {
    fn ids() -> Box<[u16]> {
        tag::Item::MINECRAFT_SPEARS.1.into()
    }
}

impl ItemBehaviour for SpearItem {
    fn normal_use_with_hand(
        &self,
        _item: &Item,
        player: &Player,
        _yaw: f32,
        _pitch: f32,
        hand: Hand,
    ) {
        if player
            .living_entity
            .item_in_use
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some()
        {
            return;
        }

        let inventory = player.inventory();
        let stack = inventory.get_stack_in_hand(hand);
        let sound = stack
            .get_data_component::<KineticWeaponImpl>()
            .and_then(|weapon| weapon.sound.clone());
        player
            .living_entity
            .set_active_hand(hand, stack, Self::USE_DURATION);
        if let Some(sound) = sound {
            player.world().play_sound_event_expect(
                player,
                &sound,
                SoundCategory::Players,
                &player.position(),
            );
        }
    }

    fn on_spear_jab(&self, stack: &ItemStack, player: &Player) {
        let Some(piercing) = stack.get_data_component::<PiercingWeaponImpl>() else {
            return;
        };
        let world = player.world();
        let Some(server) = world.server.upgrade() else {
            return;
        };

        let tps = f64::from(server.basic_config.tps);
        let attack_delay = tps / Self::attack_speed(player, stack);
        let elapsed = f64::from(player.last_attacked_ticks.load(Ordering::Acquire));
        if elapsed + 5.0 < attack_delay {
            return;
        }

        let damage = Self::attack_damage(player, stack) as f32;
        let mut hit_count = 0;
        for target in Self::targets_in_range(player, &server, stack) {
            if Self::stab_attack(
                player,
                &server,
                Hand::Right,
                stack,
                &target,
                damage,
                StabEffects {
                    damage: true,
                    knockback: piercing.deals_knockback,
                    dismount: piercing.dismounts,
                },
            ) {
                hit_count += 1;
            }
        }
        let hit_something = hit_count > 0;
        if hit_something {
            player.trigger_advancement(
                crate::entity::player::advancement::trigger::AdvancementTrigger::SpearMobs {
                    count: hit_count,
                },
            );
        }
        player.last_attacked_ticks.store(0, Ordering::Relaxed);

        let position = player.position();
        if hit_something && let Some(sound) = piercing.hit_sound.as_ref() {
            world.play_sound_event(sound, SoundCategory::Players, &position);
        }
        if let Some(sound) = piercing.sound.as_ref() {
            world.play_sound_event_expect(player, sound, SoundCategory::Players, &position);
        }
        player.swing_hand(Hand::Right, false);
    }

    fn on_use_tick(&self, stack: &ItemStack, player: &Player, remaining_use_ticks: i32) {
        let active_hand = *player
            .living_entity
            .active_hand
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(hand) = active_hand else {
            return;
        };
        let held = player.inventory().get_stack_in_hand(hand);
        if held.item.id != stack.item.id {
            player.living_entity.clear_active_hand();
            return;
        }
        Self::kinetic_attack(&held, player, hand, remaining_use_ticks);
    }

    fn get_use_duration(&self) -> i32 {
        Self::USE_DURATION
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// A target of a kinetic attack, with the damage to deal and the effects to apply.
type KineticHit = (Arc<dyn EntityBase>, f32, StabEffects);

#[derive(Clone, Copy)]
struct StabEffects {
    damage: bool,
    knockback: bool,
    dismount: bool,
}

/// Vanilla `KineticWeapon.damageEntities` `actionFactor`: mobs need a fifth of a player's speed.
const PLAYER_KINETIC_FACTOR: f64 = 1.0;
const MOB_KINETIC_FACTOR: f64 = 0.2f32 as f64;
/// Vanilla `LivingEntity.stabAttack`'s first `causeExtraKnockback`.
const MOB_STAB_KNOCKBACK: f32 = 0.4;

impl SpearItem {
    /// Vanilla `Item.getUseDuration` for a kinetic weapon.
    pub const USE_DURATION: i32 = 72_000;

    fn stab_attack(
        player: &Player,
        server: &Server,
        hand: Hand,
        stack: &ItemStack,
        target: &Arc<dyn EntityBase>,
        base_damage: f32,
        effects: StabEffects,
    ) -> bool {
        let target_entity = target.get_entity();
        let mut base_damage = base_damage;
        let mut magic_boost = Self::enchantment_damage(stack, target_entity) as f32;
        if !Self::is_using_hand(player, hand) {
            let tps = f64::from(server.basic_config.tps);
            let charge =
                player.get_attack_cooldown_progress(tps, 0.5, Self::attack_speed(player, stack))
                    as f32;
            magic_boost *= charge;
            base_damage *= charge.mul_add(charge * 0.8, 0.2);
        }

        let total_damage = if effects.damage {
            base_damage + magic_boost
        } else {
            0.0
        };
        let was_hurt = effects.damage
            && target.damage_with_context(
                target.as_ref(),
                total_damage,
                DamageType::SPEAR,
                None,
                Some(player),
                Some(player),
            );

        let config = &server.advanced_config.pvp;
        if effects.knockback && config.knockback && target.get_living_entity().is_some() {
            let attacker = player.get_entity();
            combat::handle_knockback(attacker, target.as_ref(), 0.8);
            let knockback_level = Self::knockback_level(stack);
            if knockback_level > 0 {
                combat::handle_knockback(attacker, target.as_ref(), f64::from(knockback_level));
            }
            target_entity.send_velocity();
        }

        let mut dismounted = false;
        if effects.dismount
            && let Some(vehicle) = target_entity.get_vehicle()
        {
            dismounted = true;
            vehicle
                .get_entity()
                .remove_passenger(target_entity.entity_id);
        }

        if !was_hurt && !effects.knockback && !dismounted {
            return false;
        }

        player
            .living_entity
            .last_attacking_id
            .store(target_entity.entity_id, Ordering::Relaxed);
        player.living_entity.last_attack_time.store(
            player.get_entity().tick_count.load(Ordering::Relaxed),
            Ordering::Relaxed,
        );
        if was_hurt {
            crate::enchantment::EnchantmentHelper::do_post_attack_effects_with_item_source(
                target.as_ref(),
                DamageType::SPEAR,
                Some(player),
                Some(player),
                Some(stack),
            );
        }
        if target.get_living_entity().is_some()
            && let Some(weapon) = stack.get_data_component::<WeaponImpl>()
        {
            let slot = EquipmentSlot::from_hand(hand);
            player.damage_item_in_slot(&slot, weapon.item_damage_per_attack as i32);
        }
        player.add_exhaustion(0.1);
        true
    }

    fn kinetic_attack(stack: &ItemStack, player: &Player, hand: Hand, remaining_use_ticks: i32) {
        let world = player.world();
        let Some(server) = world.server.upgrade() else {
            return;
        };
        let look = Self::look_vector(player);
        let Some(hits) = Self::kinetic_hits(
            &player.living_entity,
            stack,
            remaining_use_ticks,
            look,
            PLAYER_KINETIC_FACTOR,
            || Self::targets_in_range(player, &server, stack),
        ) else {
            return;
        };

        let mut affected = false;
        for (target, damage, effects) in hits {
            affected |= Self::stab_attack(player, &server, hand, stack, &target, damage, effects);
        }
        if affected {
            world.send_entity_status(
                player.get_entity(),
                EntityStatus::KineticHit,
                ActorEventID::KineticDamageDealt,
            );
        }
    }

    /// Vanilla `KineticWeapon.damageEntities` for a mob using a kinetic weapon, run from
    /// `ItemStack.onUseTick` for every living entity.
    pub fn mob_kinetic_attack(
        caller: &dyn EntityBase,
        stack: &ItemStack,
        remaining_use_ticks: i32,
    ) {
        let Some(living) = caller.get_living_entity() else {
            return;
        };
        let entity = &living.entity;
        let world = entity.world.load_full();
        // Vanilla `getLookAngle` scores speeds, `getHeadLookAngle` aims the hit.
        let look =
            Vector3::rotation_vector(f64::from(entity.pitch.load()), f64::from(entity.yaw.load()));
        let head_look = Vector3::rotation_vector(
            f64::from(entity.pitch.load()),
            f64::from(entity.head_yaw.load()),
        );
        let Some(hits) = Self::kinetic_hits(
            living,
            stack,
            remaining_use_ticks,
            look,
            MOB_KINETIC_FACTOR,
            || Self::mob_targets_in_range(caller, &world, stack, head_look),
        ) else {
            return;
        };

        let mut affected = false;
        for (target, damage, effects) in hits {
            affected |= Self::mob_stab_attack(caller, stack, &target, damage, effects);
        }
        if affected {
            world.send_entity_status(
                entity,
                EntityStatus::KineticHit,
                ActorEventID::KineticDamageDealt,
            );
        }
    }

    /// The shared part of vanilla `KineticWeapon.damageEntities`: the targets not stabbed within
    /// the contact cooldown that meet a condition, with the damage to deal. `None` until the
    /// weapon's delay has passed.
    fn kinetic_hits(
        attacker: &LivingEntity,
        stack: &ItemStack,
        remaining_use_ticks: i32,
        look: Vector3<f64>,
        entity_factor: f64,
        targets: impl FnOnce() -> Vec<Arc<dyn EntityBase>>,
    ) -> Option<Vec<KineticHit>> {
        let weapon = stack.get_data_component::<KineticWeaponImpl>()?;
        let ticks_used = Self::USE_DURATION - remaining_use_ticks - weapon.delay_ticks;
        if ticks_used < 0 {
            return None;
        }

        let attacker_speed = look.dot(&Self::known_speed(&attacker.entity));
        let base_damage = attacker.get_attribute_base(&Attributes::ATTACK_DAMAGE) as f32;
        let now = attacker.entity.tick_count.load(Ordering::Relaxed);
        let mut hits = Vec::new();
        for target in targets() {
            let target_entity = target.get_entity();
            if attacker.was_recently_stabbed(
                target_entity.entity_id,
                now,
                weapon.contact_cooldown_ticks,
            ) {
                continue;
            }
            attacker.remember_stabbed_entity(target_entity.entity_id, now);

            let target_speed = look.dot(&Self::known_speed(target_entity));
            let relative_speed = (attacker_speed - target_speed).max(0.0);
            let test = |condition: &Option<KineticConditionImpl>| {
                condition.as_ref().is_some_and(|condition| {
                    condition.test(ticks_used, attacker_speed, relative_speed, entity_factor)
                })
            };
            let effects = StabEffects {
                damage: test(&weapon.damage_conditions),
                knockback: test(&weapon.knockback_conditions),
                dismount: test(&weapon.dismount_conditions),
            };
            if !effects.damage && !effects.knockback && !effects.dismount {
                continue;
            }

            let damage =
                base_damage + (relative_speed * f64::from(weapon.damage_multiplier)).floor() as f32;
            hits.push((target, damage, effects));
        }
        Some(hits)
    }

    /// Vanilla `LivingEntity.stabAttack`, the version mobs use.
    fn mob_stab_attack(
        attacker: &dyn EntityBase,
        stack: &ItemStack,
        target: &Arc<dyn EntityBase>,
        base_damage: f32,
        effects: StabEffects,
    ) -> bool {
        let target_entity = target.get_entity();
        // Vanilla `EnchantmentHelper.modifyDamage`.
        let damage = base_damage + Self::enchantment_damage(stack, target_entity) as f32;
        let dealt_damage = effects.damage
            && target.damage_with_context(
                target.as_ref(),
                damage,
                DamageType::SPEAR,
                None,
                Some(attacker),
                Some(attacker),
            );
        let mut affected = effects.knockback || dealt_damage;
        if effects.knockback {
            let attacker_entity = attacker.get_entity();
            combat::cause_extra_knockback(attacker_entity, target.as_ref(), MOB_STAB_KNOCKBACK);
            // Vanilla `LivingEntity.getKnockback`.
            let knockback = attacker.get_living_entity().map_or(0.0, |living| {
                crate::enchantment::EnchantmentHelper::modify_knockback(
                    stack,
                    living.get_attribute_value(&Attributes::ATTACK_KNOCKBACK) as f32,
                ) / 2.0
            });
            combat::cause_extra_knockback(attacker_entity, target.as_ref(), knockback);
            target_entity.send_velocity();
        }

        if effects.dismount
            && !target_entity
                .entity_type
                .has_tag(&tag::EntityType::MINECRAFT_CANNOT_BE_DISMOUNTED_BY_ITEM_USAGE)
            && let Some(vehicle) = target_entity.get_vehicle()
        {
            affected = true;
            vehicle
                .get_entity()
                .remove_passenger(target_entity.entity_id);
        }

        if dealt_damage {
            crate::enchantment::EnchantmentHelper::do_post_attack_effects(
                target.as_ref(),
                DamageType::SPEAR,
                Some(attacker),
                Some(attacker),
            );
        }
        if !affected {
            return false;
        }
        // Vanilla `setLastHurtMob`.
        if let Some(living) = attacker.get_living_entity() {
            living
                .last_attacking_id
                .store(target_entity.entity_id, Ordering::Relaxed);
            living.last_attack_time.store(
                living.entity.tick_count.load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
        }
        true
    }

    fn look_vector(player: &Player) -> Vector3<f64> {
        let (yaw, pitch) = player.rotation();
        Vector3::rotation_vector(f64::from(pitch), f64::from(yaw))
    }

    fn known_speed(entity: &Entity) -> Vector3<f64> {
        let mut movement = entity.movement.load();
        if entity.entity_type != &EntityType::PLAYER {
            let mut vehicle = entity.get_vehicle();
            while let Some(current) = vehicle {
                movement = current.get_entity().movement.load();
                vehicle = current.get_entity().get_vehicle();
            }
        }
        movement * 20.0
    }

    fn is_using_hand(player: &Player, hand: Hand) -> bool {
        let living = &player.living_entity;
        living
            .item_in_use
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some()
            && *living
                .active_hand
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                == Some(hand)
    }

    fn attack_damage(player: &Player, stack: &ItemStack) -> f64 {
        // Strength and Weakness are already attack damage modifiers.
        Self::attribute_with_item_modifier(
            player,
            stack,
            &Attributes::ATTACK_DAMAGE,
            "minecraft:base_attack_damage",
        )
        .max(0.0)
    }

    fn attack_speed(player: &Player, stack: &ItemStack) -> f64 {
        Self::attribute_with_item_modifier(
            player,
            stack,
            &Attributes::ATTACK_SPEED,
            "minecraft:base_attack_speed",
        )
        .max(f64::EPSILON)
    }

    fn attribute_with_item_modifier(
        player: &Player,
        stack: &ItemStack,
        attribute: &Attributes,
        modifier_id: &str,
    ) -> f64 {
        let living = &player.living_entity;
        let value = living.get_attribute_value(attribute);
        let already_applied = living
            .attributes
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&attribute.id)
            .is_some_and(|instance| {
                instance
                    .modifiers
                    .iter()
                    .any(|modifier| modifier.id == modifier_id)
            });
        if already_applied {
            return value;
        }
        let Some(modifiers) = stack.get_data_component::<AttributeModifiersImpl>() else {
            return value;
        };
        modifiers
            .attribute_modifiers
            .iter()
            .filter(|modifier| {
                modifier.id == modifier_id && modifier.operation == Operation::AddValue
            })
            .fold(value, |value, modifier| value + modifier.amount)
    }

    fn enchantment_damage(stack: &ItemStack, target: &Entity) -> f64 {
        let Some(enchantments) = stack.get_data_component::<EnchantmentsImpl>() else {
            return 0.0;
        };
        enchantments
            .enchantment
            .iter()
            .map(|(enchantment, level)| {
                let level = f64::from(*level);
                let smite = **enchantment == Enchantment::SMITE
                    && target
                        .entity_type
                        .has_tag(&tag::EntityType::MINECRAFT_SENSITIVE_TO_SMITE);
                let bane = **enchantment == Enchantment::BANE_OF_ARTHROPODS
                    && target
                        .entity_type
                        .has_tag(&tag::EntityType::MINECRAFT_SENSITIVE_TO_BANE_OF_ARTHROPODS);
                if **enchantment == Enchantment::SHARPNESS {
                    0.5 * level + 0.5
                } else if smite || bane {
                    2.5 * level
                } else {
                    0.0
                }
            })
            .sum()
    }

    fn knockback_level(stack: &ItemStack) -> u32 {
        stack
            .get_data_component::<EnchantmentsImpl>()
            .and_then(|enchantments| {
                enchantments
                    .enchantment
                    .iter()
                    .find(|(enchantment, _)| **enchantment == Enchantment::KNOCKBACK)
                    .map(|(_, level)| u32::try_from(*level).unwrap_or(0))
            })
            .unwrap_or(0)
    }

    fn targets_in_range(
        player: &Player,
        server: &Server,
        stack: &ItemStack,
    ) -> Vec<Arc<dyn EntityBase>> {
        let Some(range) = stack.get_data_component::<AttackRangeImpl>() else {
            return Vec::new();
        };
        let (min_reach, max_reach) = if player.gamemode.load() == GameMode::Creative {
            (
                f64::from(range.min_creative_reach),
                f64::from(range.max_creative_reach),
            )
        } else {
            (f64::from(range.min_reach), f64::from(range.max_reach))
        };
        Self::entities_along(
            &player.world(),
            player.get_entity(),
            player.eye_position(),
            Self::look_vector(player),
            (min_reach, max_reach),
            f64::from(range.hitbox_margin),
            |target| Self::can_hit(player.get_entity(), Some(server), target),
        )
    }

    /// The mob reach of vanilla `AttackRange`: `effectiveMinRange`/`effectiveMaxRange` scaled by
    /// `mob_factor`, or the interaction range without an attack range component.
    fn mob_targets_in_range(
        attacker: &dyn EntityBase,
        world: &World,
        stack: &ItemStack,
        head_look: Vector3<f64>,
    ) -> Vec<Arc<dyn EntityBase>> {
        let entity = attacker.get_entity();
        let (reach, margin) = stack.get_data_component::<AttackRangeImpl>().map_or_else(
            || {
                let interaction_range = attacker.get_living_entity().map_or(0.0, |living| {
                    f64::from(
                        living.get_attribute_value(&Attributes::ENTITY_INTERACTION_RANGE) as f32,
                    )
                });
                ((0.0, interaction_range), 0.0)
            },
            |range| {
                (
                    (
                        f64::from(range.min_reach * range.mob_factor),
                        f64::from(range.max_reach * range.mob_factor),
                    ),
                    f64::from(range.hitbox_margin),
                )
            },
        );
        Self::entities_along(
            world,
            entity,
            entity.get_eye_pos(),
            head_look,
            reach,
            margin,
            |target| Self::can_hit(entity, None, target),
        )
    }

    /// Vanilla `ProjectileUtil.getHitEntitiesAlong`: the entities the attacker's look ray
    /// passes within `margin` of, between the reach bounds and in front of the first block.
    fn entities_along(
        world: &World,
        attacker: &Entity,
        eye: Vector3<f64>,
        look: Vector3<f64>,
        (min_reach, max_reach): (f64, f64),
        margin: f64,
        can_hit: impl Fn(&dyn EntityBase) -> bool,
    ) -> Vec<Arc<dyn EntityBase>> {
        let from = eye.add(&(look * min_reach));
        let forward = attacker.movement.load().dot(&look).max(0.0);
        let mut to = eye.add(&(look * (max_reach + forward)));
        if let Some(block_hit) = Self::clip_blocks(world, eye, to) {
            if eye.squared_distance_to_vec(&block_hit) < eye.squared_distance_to_vec(&from) {
                return Vec::new();
            }
            to = block_hit;
        }

        let half_margin = Vector3::new(margin / 2.0, margin / 2.0, margin / 2.0);
        let delta = to.sub(&from);
        let search_box = BoundingBox::new(from.sub(&half_margin), from.add(&half_margin))
            .expand_towards(delta.x, delta.y, delta.z)
            .expand_all(1.0);

        world
            .get_all_at_box(&search_box)
            .into_iter()
            .filter(|target| {
                can_hit(target.as_ref())
                    && Self::hits_entity(
                        world,
                        &target.get_entity().bounding_box.load(),
                        from,
                        to,
                        margin,
                    )
            })
            .collect()
    }

    fn hits_entity(
        world: &World,
        bounding_box: &BoundingBox,
        from: Vector3<f64>,
        to: Vector3<f64>,
        margin: f64,
    ) -> bool {
        if contains(bounding_box, from) || clip(bounding_box, from, to).is_some() {
            return true;
        }
        let Some(outside_hit) = clip(&bounding_box.expand_all(margin), from, to) else {
            return false;
        };
        let mut towards_target = bounding_box.min.add(&bounding_box.max) * 0.5;
        if let Some(block_hit) = Self::clip_blocks(world, outside_hit, towards_target) {
            towards_target = block_hit;
        }
        clip(bounding_box, outside_hit, towards_target).is_some()
    }

    fn clip_blocks(world: &World, from: Vector3<f64>, to: Vector3<f64>) -> Option<Vector3<f64>> {
        if from == to {
            return None;
        }
        let start = from.lerp(&to, -1.0e-7);
        let end = to.lerp(&from, -1.0e-7);
        let mut block = BlockPos::floored(start.x, start.y, start.z);
        if let Some(hit) = Self::clip_block(world, &block, from, to) {
            return Some(hit);
        }

        let difference = end.sub(&start);
        let step = difference.sign();
        let delta = Vector3::new(
            if step.x == 0 {
                f64::MAX
            } else {
                f64::from(step.x) / difference.x
            },
            if step.y == 0 {
                f64::MAX
            } else {
                f64::from(step.y) / difference.y
            },
            if step.z == 0 {
                f64::MAX
            } else {
                f64::from(step.z) / difference.z
            },
        );
        let mut next = Vector3::new(
            delta.x
                * (if step.x > 0 {
                    1.0 - (start.x - start.x.floor())
                } else {
                    start.x - start.x.floor()
                }),
            delta.y
                * (if step.y > 0 {
                    1.0 - (start.y - start.y.floor())
                } else {
                    start.y - start.y.floor()
                }),
            delta.z
                * (if step.z > 0 {
                    1.0 - (start.z - start.z.floor())
                } else {
                    start.z - start.z.floor()
                }),
        );

        while next.x <= 1.0 || next.y <= 1.0 || next.z <= 1.0 {
            if next.x < next.y && next.x < next.z {
                block.0.x += step.x;
                next.x += delta.x;
            } else if next.y < next.z {
                block.0.y += step.y;
                next.y += delta.y;
            } else {
                block.0.z += step.z;
                next.z += delta.z;
            }
            if let Some(hit) = Self::clip_block(world, &block, from, to) {
                return Some(hit);
            }
        }
        None
    }

    fn clip_block(
        world: &World,
        pos: &BlockPos,
        from: Vector3<f64>,
        to: Vector3<f64>,
    ) -> Option<Vector3<f64>> {
        world
            .get_block_state(pos)
            .get_block_collision_shapes_at(pos)
            .filter_map(|shape| clip(&shape.shift(pos.0.to_f64()), from, to))
            .min_by(|a, b| {
                from.squared_distance_to_vec(a)
                    .total_cmp(&from.squared_distance_to_vec(b))
            })
    }

    /// Vanilla `PiercingWeapon.canHitEntity`. `server` is given for a player attacker, whose
    /// hits on other players follow the `PvP` settings.
    fn can_hit(attacker: &Entity, server: Option<&Server>, target: &dyn EntityBase) -> bool {
        let entity = target.get_entity();
        if entity.entity_id == attacker.entity_id
            || entity.is_removed()
            || entity.invulnerable.load(Ordering::Relaxed)
            || target.is_spectator()
        {
            return false;
        }
        let Some(living) = target.get_living_entity() else {
            return false;
        };
        if living.health.load() <= 0.0 {
            return false;
        }
        if let (Some(server), Some(other)) = (server, target.get_player()) {
            let config = &server.advanced_config.pvp;
            if !config.enabled
                || (config.protect_creative && other.gamemode.load() == GameMode::Creative)
            {
                return false;
            }
        }
        attacker.root_vehicle_id() != entity.root_vehicle_id()
    }
}

fn contains(bounding_box: &BoundingBox, point: Vector3<f64>) -> bool {
    point.x >= bounding_box.min.x
        && point.x < bounding_box.max.x
        && point.y >= bounding_box.min.y
        && point.y < bounding_box.max.y
        && point.z >= bounding_box.min.z
        && point.z < bounding_box.max.z
}

fn clip(bounding_box: &BoundingBox, from: Vector3<f64>, to: Vector3<f64>) -> Option<Vector3<f64>> {
    let min = [bounding_box.min.x, bounding_box.min.y, bounding_box.min.z];
    let max = [bounding_box.max.x, bounding_box.max.y, bounding_box.max.z];
    let origin = [from.x, from.y, from.z];
    let delta = [to.x - from.x, to.y - from.y, to.z - from.z];
    let mut scale = 1.0;
    let mut hit = false;
    for axis in 0..3 {
        if delta[axis] > 1.0e-7 {
            hit |= clip_point(&mut scale, axis, min[axis], &origin, &delta, &min, &max);
        } else if delta[axis] < -1.0e-7 {
            hit |= clip_point(&mut scale, axis, max[axis], &origin, &delta, &min, &max);
        }
    }
    hit.then(|| from.add(&(to.sub(&from) * scale)))
}

fn clip_point(
    scale: &mut f64,
    axis: usize,
    plane: f64,
    origin: &[f64; 3],
    delta: &[f64; 3],
    min: &[f64; 3],
    max: &[f64; 3],
) -> bool {
    let s = (plane - origin[axis]) / delta[axis];
    if s <= 0.0 || s >= *scale {
        return false;
    }
    let b = (axis + 1) % 3;
    let c = (axis + 2) % 3;
    let pb = origin[b] + s * delta[b];
    let pc = origin[c] + s * delta[c];
    if min[b] - 1.0e-7 < pb && pb < max[b] + 1.0e-7 && min[c] - 1.0e-7 < pc && pc < max[c] + 1.0e-7
    {
        *scale = s;
        return true;
    }
    false
}
