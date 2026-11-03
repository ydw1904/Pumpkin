use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::EntityStatus;
use pumpkin_data::sound::Sound;
use pumpkin_protocol::bedrock::server::ActorEventID;

use crate::{
    enchantment::EnchantmentHelper,
    entity::{Entity, EntityBase, living::LivingEntity, projectile::owner::EntityReference},
    server::Server,
};

pub struct EvokerFangsEntity {
    pub entity: Entity,
    pub warmup_delay_ticks: AtomicI32,
    pub life_ticks: AtomicI32,
    pub owner: EntityReference,
    pub sent_spike_event: AtomicBool,
}

impl EvokerFangsEntity {
    #[must_use]
    pub fn new(entity: Entity, warmup_ticks: u32, yaw: f32, owner: Option<&Entity>) -> Self {
        entity.set_rotation(yaw, 0.0);
        Self {
            entity,
            warmup_delay_ticks: AtomicI32::new(warmup_ticks as i32),
            life_ticks: AtomicI32::new(22),
            owner: EntityReference::new(owner),
            sent_spike_event: AtomicBool::new(false),
        }
    }

    /// Vanilla `EvokerFangs.dealDamageTo`.
    fn deal_damage_to(
        &self,
        target: &dyn EntityBase,
        living: &LivingEntity,
        owner: Option<&dyn EntityBase>,
    ) {
        if !living.is_alive()
            || living.entity.invulnerable.load(Ordering::Relaxed)
            || owner.is_some_and(|owner| owner.get_entity().entity_id == living.entity.entity_id)
        {
            return;
        }
        let Some(owner) = owner else {
            target.damage(target, 6.0, DamageType::MAGIC);
            return;
        };
        if owner.is_allied_to(target) {
            return;
        }
        if target.damage_with_context(
            target,
            6.0,
            DamageType::INDIRECT_MAGIC,
            None,
            Some(self),
            Some(owner),
        ) {
            EnchantmentHelper::do_post_attack_effects(
                target,
                DamageType::INDIRECT_MAGIC,
                Some(owner),
                Some(self),
            );
        }
    }
}

impl EntityBase for EvokerFangsEntity {
    fn write_custom_nbt(&self, nbt: &mut pumpkin_nbt::compound::NbtCompound) {
        nbt.put_int("Warmup", self.warmup_delay_ticks.load(Ordering::Relaxed));
        self.owner.write_nbt(nbt, "Owner");
    }

    fn read_custom_nbt(&self, nbt: &pumpkin_nbt::compound::NbtCompound) {
        if let Some(warmup) = nbt.get_int("Warmup") {
            self.warmup_delay_ticks.store(warmup, Ordering::Relaxed);
        }
        self.owner.read_nbt(nbt, "Owner");
    }

    fn remove_custom_nbt(&self, removed: &[Box<str>]) {
        if removed.iter().any(|key| &**key == "Owner") {
            self.owner.clear();
        }
    }

    fn tick(&self, _caller: &dyn EntityBase, _server: &Server) {
        let entity = &self.entity;
        let world = entity.world.load();

        let warmup = self.warmup_delay_ticks.fetch_sub(1, Ordering::Relaxed) - 1;
        if warmup < 0 {
            if warmup == -8 {
                let bb = entity.bounding_box.load().expand(0.2, 0.0, 0.2);
                let candidates = world.get_entities_and_players_at_box(&bb);

                // Vanilla `EvokerFangs.getOwner` only resolves living owners.
                let owner = self
                    .owner
                    .get(&world)
                    .filter(|owner| owner.get_living_entity().is_some());

                for cand in candidates {
                    let cand_ent = cand.get_entity();
                    if cand_ent.entity_id != entity.entity_id
                        && let Some(living) = cand.get_living_entity()
                    {
                        self.deal_damage_to(cand.as_ref(), living, owner.as_deref());
                    }
                }
            }

            if !self.sent_spike_event.swap(true, Ordering::SeqCst) {
                world.send_entity_status(
                    entity,
                    EntityStatus::StartAttacking,
                    ActorEventID::StartAttacking,
                );
                if !entity.silent.load(Ordering::Relaxed) {
                    entity.play_sound(Sound::EntityEvokerFangsAttack);
                }
            }

            let life = self.life_ticks.fetch_sub(1, Ordering::Relaxed) - 1;
            if life < 0 {
                entity.remove();
            }
        }
    }

    fn get_entity(&self) -> &Entity {
        &self.entity
    }

    fn get_living_entity(&self) -> Option<&crate::entity::living::LivingEntity> {
        None
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }
}
