use crate::entity::projectile::owner::ProjectileOwner;
use pumpkin_util::random::gameplay::game_random;
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

use crate::entity::projectile::ProjectileHit;
use crate::{
    entity::{
        Entity, EntityBase, EntityType, RemovalReason, mob::endermite::EndermiteEntity,
        projectile::ThrownItemEntity,
    },
    server::Server,
    world::TimedTicketType,
};
use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::{EntityPose, EntityStatus};
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::particle::Particle;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_protocol::bedrock::server::actor_event::ActorEventID;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

const GRAVITY: f64 = 0.03;
const PARTICLE_COUNT: i32 = 32;
const ENDERMITE_SPAWN_CHANCE: f32 = 0.05;

pub struct EnderPearlEntity {
    pub thrown: ThrownItemEntity,
    pub item_stack: RwLock<ItemStack>,
    /// Vanilla `ticketTimer`: ticks until the pearl renews its chunk ticket.
    ticket_timer: AtomicI64,
}

impl EnderPearlEntity {
    pub fn new(entity: Entity) -> Self {
        entity.set_velocity(Vector3::new(0.0, 0.1, 0.0));

        let thrown = ThrownItemEntity {
            entity,
            owner: ProjectileOwner::none(),
            has_hit: AtomicBool::new(false),
            last_deflected_by: crate::entity::projectile_deflection::LastDeflectedBy::new(),
            gravity: GRAVITY,
        };

        Self {
            thrown,
            item_stack: RwLock::new(ItemStack::new(1, &Item::ENDER_PEARL)),
            ticket_timer: AtomicI64::new(0),
        }
    }

    pub fn new_shot(entity: Entity, shooter: &Entity) -> Self {
        let thrown = ThrownItemEntity::new(entity, shooter, GRAVITY);
        thrown.entity.set_velocity(Vector3::new(0.0, 0.1, 0.0));
        Self {
            thrown,
            item_stack: RwLock::new(ItemStack::new(1, &Item::ENDER_PEARL)),
            ticket_timer: AtomicI64::new(0),
        }
    }

    pub fn set_item_stack(&self, item_stack: ItemStack) {
        *self
            .item_stack
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = item_stack;
    }
}

impl EntityBase for EnderPearlEntity {
    /// Vanilla `ThrownEnderpearl` takes the capped push other entities get.
    fn on_above_bubble_column(&self, drag_down: bool, pos: &BlockPos) {
        crate::block::blocks::bubble_column::handle_on_above_bubble_column(
            &self.thrown.entity,
            drag_down,
            pos,
        );
    }

    fn on_inside_bubble_column(&self, drag_down: bool) {
        crate::block::blocks::bubble_column::handle_on_inside_bubble_column(
            &self.thrown.entity,
            drag_down,
        );
    }

    /// Vanilla compares item stacks by identity, so the item is never at its default and always goes out with the spawn packet.
    fn init_data_tracker(&self) {
        self.get_entity().set_synced_data(
            pumpkin_data::tracked_data::ender_pearl::DATA_ITEM_STACK,
            pumpkin_protocol::codec::item_stack_seralizer::ItemStackSerializer::from(
                self.item_stack
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone(),
            ),
        );
    }

    fn get_projectile_owner(&self) -> Option<&ProjectileOwner> {
        Some(&self.thrown.owner)
    }

    fn write_custom_nbt(&self, nbt: &mut pumpkin_nbt::compound::NbtCompound) {
        super::write_thrown_item(&self.item_stack, nbt);
    }

    fn read_custom_nbt(&self, nbt: &pumpkin_nbt::compound::NbtCompound) {
        self.set_item_stack(super::read_thrown_item(
            self.get_entity(),
            nbt,
            &Item::ENDER_PEARL,
            pumpkin_data::tracked_data::ender_pearl::DATA_ITEM_STACK,
        ));
    }

    /// Vanilla `ThrownEnderpearl.tick`.
    fn tick(&self, caller: &dyn EntityBase, _server: &Server) {
        let entity = self.get_entity();
        let world = entity.world.load_full();
        let previous_chunk = BlockPos::floored_v(entity.pos.load()).chunk_position();
        let owner = self.thrown.owner.get_player(&world);
        if let Some(owner) = &owner
            && !owner.living_entity.is_alive()
            && !owner.won_game.load(Ordering::Relaxed)
            && owner
                .world()
                .level_info
                .load()
                .game_rules
                .ender_pearls_vanish_on_death
        {
            entity.remove();
        } else {
            self.thrown.process_tick(caller);
        }

        if entity.is_alive() {
            let chunk = BlockPos::floored_v(entity.pos.load()).chunk_position();
            let timer = self.ticket_timer.fetch_sub(1, Ordering::Relaxed) - 1;
            if (timer <= 0 || chunk != previous_chunk)
                && let Some(owner) = &owner
            {
                // Vanilla `ServerPlayer.registerAndUpdateEnderPearlTicket`.
                owner.register_ender_pearl(entity.entity_uuid);
                world.add_timed_ticket(TimedTicketType::EnderPearl, chunk);
                self.ticket_timer
                    .store(TimedTicketType::EnderPearl.timeout() - 1, Ordering::Relaxed);
            }
        }
    }

    /// Vanilla `ThrownEnderpearl.teleport` holds the chunks it arrives in with a portal ticket.
    fn after_teleport(&self) {
        let entity = self.get_entity();
        entity.world.load().add_timed_ticket(
            TimedTicketType::Portal,
            BlockPos::floored_v(entity.pos.load()).chunk_position(),
        );
    }

    /// Vanilla `ThrownEnderpearl.onRemoval`: the pearl stays registered with an owner who
    /// logged out with it. Vanilla's copy in the new dimension registers itself again; here the
    /// same pearl arrives, so it stays registered.
    fn on_remove(&self, reason: RemovalReason) {
        if matches!(
            reason,
            RemovalReason::UnloadedWithPlayer | RemovalReason::ChangedDimension
        ) {
            return;
        }
        let entity = self.get_entity();
        if let Some(owner) = self.thrown.owner.get_player(&entity.world.load()) {
            owner.deregister_ender_pearl(entity.entity_uuid);
        }
    }

    fn get_entity(&self) -> &Entity {
        self.thrown.get_entity()
    }

    fn get_living_entity(&self) -> Option<&crate::entity::living::LivingEntity> {
        None
    }
    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }

    fn on_hit(&self, hit: ProjectileHit) {
        let entity = self.get_entity();
        let world = entity.world.load();

        let attacker = self.get_owner_entity();

        // Spawn portal particles at hit position
        let hit_pos = hit.hit_pos();
        for _ in 0..PARTICLE_COUNT {
            let offset = Vector3::new(
                game_random().random::<f32>() as f64 - 0.5,
                game_random().random::<f32>() as f64 * 2.0,
                game_random().random::<f32>() as f64 - 0.5,
            );
            let speed = Vector3::new(
                game_random().random::<f64>() - 0.5,
                0.0,
                game_random().random::<f64>() - 0.5,
            );

            world.spawn_particle(
                hit_pos.add(&offset),
                Vector3::new(speed.x as f32, speed.y as f32, speed.z as f32),
                1.0,
                1,
                Particle::Portal,
            );
        }

        let teleport_pos = entity.last_pos.load();

        // Like vanilla, a zero hit from the pearl, credited to its thrower if any.
        if let ProjectileHit::Entity {
            entity: hit_entity,
            hit_pos,
            ..
        } = &hit
        {
            hit_entity.damage_with_context(
                &**hit_entity,
                0.0,
                DamageType::THROWN,
                Some(*hit_pos),
                Some(self),
                attacker.as_deref(),
            );
        }

        if let Some(owner) = attacker
            && owner.get_entity().is_alive()
            && owner.get_living_entity().is_none_or(|living| {
                living.health.load() > 0.0 && owner.get_entity().pose.load() != EntityPose::Sleeping
            })
        {
            let is_player = owner.get_player().is_some();
            if is_player {
                let should_spawn_endermite = game_random().random::<f32>() < ENDERMITE_SPAWN_CHANCE;
                if world.should_spawn_monsters() && should_spawn_endermite {
                    let entity = Entity::new(
                        world.clone(),
                        owner.get_entity().pos.load(),
                        &EntityType::ENDERMITE,
                    );
                    // Vanilla `snapTo` the owner's rotation; `EntityType.create` doesn't finalize.
                    entity.set_rotation(
                        owner.get_entity().yaw.load(),
                        owner.get_entity().pitch.load(),
                    );
                    let endermite = EndermiteEntity::new(entity);
                    world.spawn_entity(endermite);
                }
            }

            // In vanilla, teleport handles everything including sound
            owner.get_entity().stop_riding_for_teleport();
            owner.teleport(
                teleport_pos,
                Some(owner.get_entity().yaw.load()),
                Some(owner.get_entity().pitch.load()),
                world.clone(),
            );

            // Play teleport sound at new position
            world.play_sound(
                Sound::EntityPlayerTeleport,
                SoundCategory::Players,
                &teleport_pos,
            );

            if let Some(living) = owner.get_living_entity() {
                living.fall_distance.store(0.0);
                living.reset_current_impulse_context();
            }
            if is_player {
                owner.damage(
                    owner.as_ref(),
                    5.0,
                    pumpkin_data::damage::DamageType::ENDER_PEARL,
                );
            }
        }

        world.send_entity_status(entity, EntityStatus::Death, ActorEventID::Death);
    }
}
