use pumpkin_data::attributes::Attributes;
use pumpkin_data::damage::DamageType;
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::entity::EntityStatus;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_protocol::bedrock::server::ActorEventID;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

use super::GoatEntity;
use crate::enchantment::helper::EnchantmentHelper;
use crate::entity::EntityBase;
use crate::entity::ai::brain::{
    BrainTick,
    behavior::{Behavior, prepare_ram_nearest_target::RamTargeting},
    memory::{MemoryModuleId, MemoryStatus, types, walk_target::WalkTarget},
};

const TIME_OUT_DURATION: i32 = 200;
const RAM_SPEED_FORCE_FACTOR: f32 = 1.65;

/// Charges at `RAM_TARGET`, hurting and knocking back the first entity it touches, or losing a
/// horn when it hits a `#snaps_goat_horn` block.
pub struct RamTarget {
    get_time_between_rams: fn(&GoatEntity) -> (i32, i32),
    ram_targeting: RamTargeting,
    speed: f32,
    get_knockback_force: fn(&GoatEntity) -> f64,
    get_impact_sound: fn(&GoatEntity) -> Sound,
    horn_break_sound: Sound,
    ram_direction: Vector3<f64>,
}

impl RamTarget {
    #[must_use]
    pub const fn new(
        get_time_between_rams: fn(&GoatEntity) -> (i32, i32),
        ram_targeting: RamTargeting,
        speed: f32,
        get_knockback_force: fn(&GoatEntity) -> f64,
        get_impact_sound: fn(&GoatEntity) -> Sound,
        horn_break_sound: Sound,
    ) -> Self {
        Self {
            get_time_between_rams,
            ram_targeting,
            speed,
            get_knockback_force,
            get_impact_sound,
            horn_break_sound,
            ram_direction: Vector3::new(0.0, 0.0, 0.0),
        }
    }

    fn finish_ram(&self, tick: &mut BrainTick<'_>, goat: &GoatEntity) {
        tick.world.send_entity_status(
            tick.mob.get_entity(),
            EntityStatus::EndRam,
            ActorEventID::None,
        );
        let (min, max) = (self.get_time_between_rams)(goat);
        let cooldown = tick.mob.get_random().random_range(min..=max);
        tick.state.set(types::RAM_COOLDOWN_TICKS, cooldown);
        tick.state.erase(types::RAM_TARGET.id());
    }

    fn play_sound(tick: &BrainTick<'_>, sound: Sound) {
        tick.world.play_sound_fine(
            sound,
            SoundCategory::Neutral,
            &tick.mob.get_entity().pos.load(),
            1.0,
            1.0,
        );
    }

    fn ram(&self, goat: &GoatEntity, target: &dyn EntityBase) {
        let Some(target_living) = target.get_living_entity() else {
            return;
        };
        let living = &goat.mob_entity.living_entity;
        let damage = living.get_attribute_value(&Attributes::ATTACK_DAMAGE) as f32;
        if target.damage_with_context(
            target,
            damage,
            DamageType::MOB_ATTACK_NO_AGGRO,
            None,
            Some(goat),
            Some(goat),
        ) {
            EnchantmentHelper::do_post_attack_effects(
                target,
                DamageType::MOB_ATTACK_NO_AGGRO,
                Some(goat),
                Some(goat),
            );
        }
        let effect_level = |effect: &'static StatusEffect| {
            living
                .get_effect(effect)
                .map_or(0, |effect| i32::from(effect.amplifier) + 1)
        };
        let speed_boost_power = 0.25
            * (effect_level(&StatusEffect::SPEED) - effect_level(&StatusEffect::SLOWNESS)) as f32;
        let speed_factor =
            (living.get_speed() * RAM_SPEED_FORCE_FACTOR).clamp(0.2, 3.0) + speed_boost_power;
        let (blocked_damage, _) = target_living.apply_item_blocking(
            target,
            damage,
            DamageType::MOB_ATTACK,
            None,
            Some(goat),
        );
        let blocking_factor = if blocked_damage > 0.0 { 0.5 } else { 1.0 };
        target_living.knockback(
            target,
            f64::from(blocking_factor * speed_factor) * (self.get_knockback_force)(goat),
            self.ram_direction.x,
            self.ram_direction.z,
            Some(goat.get_entity()),
            damage,
        );
    }

    fn has_rammed_horn_breaking_block(tick: &BrainTick<'_>) -> bool {
        let entity = tick.mob.get_entity();
        let velocity = entity.velocity.load();
        let horizontal = Vector3::new(velocity.x, 0.0, velocity.z).normalize();
        let facing = BlockPos::floored_v(entity.pos.load() + horizontal);
        tick.world
            .get_block(&facing)
            .has_tag(&tag::Block::MINECRAFT_SNAPS_GOAT_HORN)
            || tick
                .world
                .get_block(&facing.up())
                .has_tag(&tag::Block::MINECRAFT_SNAPS_GOAT_HORN)
    }
}

impl Behavior for RamTarget {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        const CONDITIONS: [(MemoryModuleId, MemoryStatus); 2] = [
            (types::RAM_COOLDOWN_TICKS.id(), MemoryStatus::ValueAbsent),
            (types::RAM_TARGET.id(), MemoryStatus::ValuePresent),
        ];
        &CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        TIME_OUT_DURATION
    }

    fn max_duration(&self) -> i32 {
        TIME_OUT_DURATION
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        tick.state.has_memory_value(types::RAM_TARGET.id())
    }

    fn can_still_use(&mut self, tick: &mut BrainTick<'_>) -> bool {
        tick.state.has_memory_value(types::RAM_TARGET.id())
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        let Some(ram_target_pos) = tick.state.get(types::RAM_TARGET).copied() else {
            return;
        };
        let cur_pos = tick.mob.get_entity().block_pos.load();
        self.ram_direction = Vector3::new(
            f64::from(cur_pos.0.x) - ram_target_pos.x,
            0.0,
            f64::from(cur_pos.0.z) - ram_target_pos.z,
        )
        .normalize();
        tick.state.set(
            types::WALK_TARGET,
            WalkTarget::from_vec(ram_target_pos, self.speed, 0),
        );
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        let Some(goat) = tick.mob.cast_any().downcast_ref::<GoatEntity>() else {
            return;
        };
        let bounding_box = tick.mob.get_entity().bounding_box.load();
        let ram_target = tick
            .world
            .get_all_at_box(&bounding_box)
            .into_iter()
            .find(|entity| {
                entity.get_living_entity().is_some()
                    && (self.ram_targeting)(tick.world, tick.mob, entity.as_ref())
            });
        if let Some(ram_target) = ram_target {
            self.ram(goat, ram_target.as_ref());
            self.finish_ram(tick, goat);
            Self::play_sound(tick, (self.get_impact_sound)(goat));
        } else if Self::has_rammed_horn_breaking_block(tick) {
            Self::play_sound(tick, (self.get_impact_sound)(goat));
            if goat.drop_horn() {
                Self::play_sound(tick, self.horn_break_sound);
            }
            self.finish_ram(tick, goat);
        } else {
            let lost_or_reached_target = match (
                tick.state.get(types::WALK_TARGET),
                tick.state.get(types::RAM_TARGET),
            ) {
                (Some(walk_target), Some(ram_target)) => {
                    walk_target
                        .target()
                        .current_position()
                        .squared_distance_to_vec(ram_target)
                        < 0.25 * 0.25
                }
                _ => true,
            };
            if lost_or_reached_target {
                self.finish_ram(tick, goat);
            }
        }
    }

    fn debug_name(&self) -> &'static str {
        "RamTarget"
    }
}
