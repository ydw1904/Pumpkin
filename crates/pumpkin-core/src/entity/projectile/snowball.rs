use crate::entity::projectile::owner::ProjectileOwner;
use std::sync::RwLock;
use std::sync::atomic::AtomicBool;

use crate::entity::projectile::ProjectileHit;
use crate::{
    entity::{Entity, EntityBase, projectile::ThrownItemEntity},
    server::Server,
};
use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::{EntityStatus, EntityType};
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_protocol::bedrock::server::actor_event::ActorEventID;

const GRAVITY: f64 = 0.03;

pub struct SnowballEntity {
    pub thrown: ThrownItemEntity,
    pub item_stack: RwLock<ItemStack>,
}

impl SnowballEntity {
    pub fn new(entity: Entity) -> Self {
        let thrown = ThrownItemEntity {
            entity,
            owner: ProjectileOwner::none(),
            has_hit: AtomicBool::new(false),
            last_deflected_by: crate::entity::projectile_deflection::LastDeflectedBy::new(),
            gravity: GRAVITY,
        };

        Self {
            thrown,
            item_stack: RwLock::new(ItemStack::new(1, &Item::SNOWBALL)),
        }
    }

    pub fn new_shot(entity: Entity, shooter: &Entity) -> Self {
        let thrown = ThrownItemEntity::new(entity, shooter, GRAVITY);
        Self {
            thrown,
            item_stack: RwLock::new(ItemStack::new(1, &Item::SNOWBALL)),
        }
    }

    pub fn set_item_stack(&self, item_stack: ItemStack) {
        *self
            .item_stack
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = item_stack;
    }
}

impl EntityBase for SnowballEntity {
    /// Vanilla compares item stacks by identity, so the item is never at its default and always goes out with the spawn packet.
    fn init_data_tracker(&self) {
        self.get_entity().set_synced_data(
            pumpkin_data::tracked_data::snowball::DATA_ITEM_STACK,
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
            &Item::SNOWBALL,
            pumpkin_data::tracked_data::snowball::DATA_ITEM_STACK,
        ));
    }

    fn tick(&self, caller: &dyn EntityBase, _server: &Server) {
        self.thrown.process_tick(caller);
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

    fn on_hit(&self, hit: crate::entity::projectile::ProjectileHit) {
        let world = self.get_entity().world.load();

        // Vanilla `Snowball.onHitEntity`: a thrown hit from the snowball, credited to its
        // thrower, so even the zero damage most mobs take knocks them back.
        if let ProjectileHit::Entity { ref entity, .. } = hit {
            let is_blaze = entity.get_entity().entity_type.id == EntityType::BLAZE.id;
            let damage = if is_blaze { 3.0 } else { 0.0 };
            let owner = self.get_owner_entity();
            entity.damage_with_context(
                entity.as_ref(),
                damage,
                DamageType::THROWN,
                None,
                Some(self),
                owner.as_deref(),
            );
        }

        world.send_entity_status(self.get_entity(), EntityStatus::Death, ActorEventID::Death);
    }
}
