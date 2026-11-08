use crate::entity::projectile::owner::ProjectileOwner;
use std::sync::atomic::{AtomicBool, Ordering};

use pumpkin_data::Block;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::world::BlockFlags;

use crate::{
    block::blocks::fire::{FireBlockBase, fire::FireBlock},
    entity::{
        Entity, EntityBase,
        projectile::{
            AccelerationPower, HURTING_INERTIA, HURTING_LIQUID_INERTIA, ProjectileHit,
            ThrownItemEntity,
            fireball::{FireballItem, INITIAL_ACCELERATION_POWER, fireball_damage_type},
        },
    },
    server::Server,
};

const GRAVITY: f64 = 0.0;

pub struct SmallFireballEntity {
    pub thrown: ThrownItemEntity,
    pub item: FireballItem,
    pub acceleration_power: AccelerationPower,
}

impl SmallFireballEntity {
    #[must_use]
    pub fn new(entity: Entity) -> Self {
        let thrown = ThrownItemEntity {
            entity,
            owner: ProjectileOwner::none(),
            collides_with_projectiles: false,
            has_hit: AtomicBool::new(false),
            last_deflected_by: crate::entity::projectile_deflection::LastDeflectedBy::new(),
            gravity: GRAVITY,
        };

        Self {
            thrown,
            item: FireballItem::new(),
            acceleration_power: AccelerationPower::new(INITIAL_ACCELERATION_POWER),
        }
    }

    #[must_use]
    pub fn new_shot(entity: Entity, shooter: &Entity, direction: Vector3<f64>) -> Self {
        let thrown = ThrownItemEntity::new(entity, shooter, GRAVITY);
        let accel = INITIAL_ACCELERATION_POWER;
        thrown
            .entity
            .velocity
            .store(direction.normalize().multiply(accel, accel, accel));
        Self {
            thrown,
            item: FireballItem::new(),
            acceleration_power: AccelerationPower::new(INITIAL_ACCELERATION_POWER),
        }
    }
}

impl EntityBase for SmallFireballEntity {
    fn init_data_tracker(&self) {
        self.item.init_data_tracker(self.get_entity());
    }

    fn on_deflection(&self, by_attack: bool) {
        self.acceleration_power.on_deflection(by_attack);
    }

    fn write_custom_nbt(&self, nbt: &mut NbtCompound) {
        self.acceleration_power.write_nbt(nbt);
        self.item.write_nbt(nbt);
    }

    fn read_custom_nbt(&self, nbt: &NbtCompound) {
        self.acceleration_power.read_nbt(nbt);
        self.item.read_nbt(self.get_entity(), nbt);
    }

    fn get_projectile_owner(&self) -> Option<&ProjectileOwner> {
        Some(&self.thrown.owner)
    }

    fn tick(&self, caller: &dyn EntityBase, _server: &Server) {
        self.thrown.process_hurting_tick(
            caller,
            self.acceleration_power.get(),
            HURTING_INERTIA,
            HURTING_LIQUID_INERTIA,
            true,
        );
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
        match hit {
            ProjectileHit::Entity { ref entity, .. } => {
                let world = self.get_entity().world.load();
                let owner = self.thrown.owner.get(&world);
                let target = entity.get_entity();
                let remaining_fire_ticks = target.fire_ticks.load(Ordering::Relaxed);
                target.set_on_fire_for(5.0);
                let damage_type = fireball_damage_type(owner.as_deref());
                if entity.damage_with_context(
                    entity.as_ref(),
                    5.0,
                    damage_type,
                    None,
                    Some(self),
                    owner.as_deref(),
                ) {
                    crate::enchantment::EnchantmentHelper::do_post_attack_effects(
                        entity.as_ref(),
                        damage_type,
                        owner.as_deref(),
                        Some(self),
                    );
                } else {
                    target
                        .fire_ticks
                        .store(remaining_fire_ticks, Ordering::Relaxed);
                }
            }
            ProjectileHit::Block { pos, face, .. } => {
                let world = self.get_entity().world.load_full();
                let owner = self.thrown.owner.get(&world);
                if owner.as_deref().and_then(EntityBase::get_mob).is_some()
                    && !world.level_info.load().game_rules.mob_griefing
                {
                    return;
                }
                let fire_pos = pos.offset(face.to_offset());
                if world.get_block_state(&fire_pos).is_air() {
                    // Vanilla `BaseFireBlock.getState`.
                    let fire_block = FireBlockBase::get_fire_type(&world, &fire_pos);
                    let state_id = if fire_block == Block::FIRE {
                        FireBlock.get_state_for_position(&world, &fire_block, &fire_pos)
                    } else {
                        fire_block.default_state.id
                    };
                    world.set_block_state(&fire_pos, state_id, BlockFlags::NOTIFY_ALL);
                }
            }
        }
    }
}
