use crate::entity::projectile::owner::ProjectileOwner;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, RwLock};

use crate::block::blocks::{campfire::CampfireBlock, candles};
use crate::entity::projectile::ProjectileHit;
use crate::{
    entity::{Entity, EntityBase, mob::Mob, projectile::ThrownItemEntity},
    server::Server,
};
use pumpkin_data::BlockDirection;
use pumpkin_data::block_properties::{
    CampfireLikeProperties, CandleCakeProperties, CandleLikeProperties,
};
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::world::WorldEvent;
use pumpkin_protocol::java::client::play::CWorldEvent;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::math::{boundingbox::BoundingBox, vector2::Vector2};
use pumpkin_util::math::{position::BlockPos, vector2::to_chunk_pos};
use pumpkin_world::world::BlockFlags;

const GRAVITY: f64 = 0.05;

pub struct SplashPotionEntity {
    pub thrown: ThrownItemEntity,
    pub item_stack: RwLock<ItemStack>,
}

impl SplashPotionEntity {
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
            item_stack: RwLock::new(ItemStack::new(1, &pumpkin_data::item::Item::SPLASH_POTION)),
        }
    }

    pub fn new_shot(entity: Entity, shooter: &Entity) -> Self {
        let thrown = ThrownItemEntity::new(entity, shooter, GRAVITY);
        thrown.entity.set_velocity(Vector3::new(0.0, 0.1, 0.0));
        Self {
            thrown,
            item_stack: RwLock::new(ItemStack::new(1, &pumpkin_data::item::Item::SPLASH_POTION)),
        }
    }

    pub fn set_item_stack(&self, item_stack: ItemStack) {
        let mut write = self
            .item_stack
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *write = item_stack;
    }
}

pub(crate) fn is_water_potion(stack: &ItemStack) -> bool {
    stack
        .get_data_component::<pumpkin_data::data_component_impl::PotionContentsImpl>()
        .and_then(|pc| pc.potion_id)
        == Some(pumpkin_data::potion::Potion::WATER.id as i32)
}

/// Vanilla `AbstractThrownPotion.onHitBlock`: a potion in `douses_fire` that hits a block puts
/// out the block in front of the hit face, the hit block and the four blocks beside the first.
pub(crate) fn douse_fire_on_block_hit(
    world: &Arc<crate::world::World>,
    projectile: &dyn EntityBase,
    hit: &ProjectileHit,
    stack: &ItemStack,
) {
    let ProjectileHit::Block { pos, face, .. } = hit else {
        return;
    };
    let douses_fire = stack
        .get_data_component::<pumpkin_data::data_component_impl::PotionContentsImpl>()
        .and_then(|pc| pc.potion_id)
        .and_then(|id| u16::try_from(id).ok())
        .is_some_and(|id| {
            pumpkin_data::datapack_tags::resolve(&pumpkin_data::tag::Potion::MINECRAFT_DOUSES_FIRE)
                .1
                .contains(&id)
        });
    if !douses_fire {
        return;
    }
    let effect_pos = pos.offset(face.to_offset());
    douse_fire(world, projectile, effect_pos);
    douse_fire(world, projectile, *pos);
    for direction in [
        BlockDirection::North,
        BlockDirection::East,
        BlockDirection::South,
        BlockDirection::West,
    ] {
        douse_fire(world, projectile, effect_pos.offset(direction.to_offset()));
    }
}

/// Vanilla `AbstractThrownPotion.douseFire`: fire is destroyed, lit candles and campfires go out.
fn douse_fire(world: &Arc<crate::world::World>, projectile: &dyn EntityBase, pos: BlockPos) {
    let (block, state) = world.get_block_and_state(&pos);
    if block.has_tag(&tag::Block::MINECRAFT_FIRE) {
        world.destroy_block(
            &pos,
            Some(projectile),
            BlockFlags::NOTIFY_ALL | BlockFlags::SKIP_DROPS,
        );
    } else if block.has_tag(&tag::Block::MINECRAFT_CANDLES) {
        let mut properties = CandleLikeProperties::from_state_id(state.id);
        if properties.lit {
            properties.lit = false;
            candles::extinguish(world, &pos, properties.to_state_id(block), None);
        }
    } else if block.has_tag(&tag::Block::MINECRAFT_CANDLE_CAKES) {
        let mut properties = CandleCakeProperties::from_state_id(state.id);
        if properties.lit {
            properties.lit = false;
            candles::extinguish(world, &pos, properties.to_state_id(block), None);
        }
    } else if CampfireBlock::is_lit_campfire(state) {
        world.sync_world_event(WorldEvent::SoundExtinguishFire, pos, 0);
        let owner = projectile.get_owner_entity();
        CampfireBlock::douse(world, &pos, owner.as_deref());
        let mut properties = CampfireLikeProperties::from_state_id(state.id);
        properties.lit = false;
        world.set_block_state(&pos, properties.to_state_id(block), BlockFlags::NOTIFY_ALL);
    }
}

/// Hurts water-sensitive mobs around `hit_pos`, mirroring the
/// `HURTS_WATER_SENSITIVE_ENTITIES` branch of vanilla `AbstractThrownPotion#affectEntitiesAround`.
/// `kept_ids` restricts the candidates so a plugin can prune them.
pub(crate) fn hurt_water_sensitive_entities(
    world: &Arc<crate::world::World>,
    projectile: &dyn EntityBase,
    hit_pos: Vector3<f64>,
    kept_ids: Option<&[i32]>,
) {
    let owner = projectile.get_owner_entity();

    // Vanilla moves the projectile onto the hit location before `onHit`, so the
    // box and distance are measured from `hit_pos`, not the entity's position.
    let entity = projectile.get_entity();
    let hurt_box = entity
        .bounding_box
        .load()
        .shift(hit_pos.sub(&entity.pos.load()))
        .expand(4.0, 2.0, 4.0);

    for candidate in world.get_entities_and_players_at_box(&hurt_box) {
        let entity_id = candidate.get_entity().entity_id;
        if let Some(kept_ids) = kept_ids
            && !kept_ids.contains(&entity_id)
        {
            continue;
        }
        if !candidate.get_mob().is_some_and(Mob::is_sensitive_to_water) {
            continue;
        }
        if hit_pos.squared_distance_to_vec(&candidate.get_entity().pos.load()) >= 16.0 {
            continue;
        }

        let _ = candidate.damage_with_context(
            candidate.as_ref(),
            1.0,
            pumpkin_data::damage::DamageType::INDIRECT_MAGIC,
            None,
            Some(entity),
            owner.as_deref(),
        );
    }
}

/// The `REHYDRATES_AXOLOTLS` branch of vanilla `AbstractThrownPotion#affectEntitiesAround`:
/// every axolotl in the box, with no distance check.
pub(crate) fn rehydrate_axolotls(
    world: &Arc<crate::world::World>,
    projectile: &dyn EntityBase,
    hit_pos: Vector3<f64>,
) {
    let entity = projectile.get_entity();
    let rehydrate_box = entity
        .bounding_box
        .load()
        .shift(hit_pos.sub(&entity.pos.load()))
        .expand(4.0, 2.0, 4.0);
    for candidate in world.get_entities_at_box(&rehydrate_box) {
        if let Some(axolotl) = candidate
            .cast_any()
            .downcast_ref::<crate::entity::passive::axolotl::AxolotlEntity>()
        {
            axolotl.rehydrate();
        }
    }
}

impl EntityBase for SplashPotionEntity {
    fn get_projectile_owner(&self) -> Option<&ProjectileOwner> {
        Some(&self.thrown.owner)
    }

    fn init_data_tracker(&self) {
        let entity = self.get_entity();
        let stack = self
            .item_stack
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        // Sync the item stack
        entity.set_synced_data(
            pumpkin_data::tracked_data::splash_potion::ITEM_STACK,
            pumpkin_protocol::codec::item_stack_seralizer::ItemStackSerializer::from(stack.clone()),
        );
    }

    fn write_custom_nbt(&self, nbt: &mut pumpkin_nbt::compound::NbtCompound) {
        super::write_thrown_item(&self.item_stack, nbt);
    }

    fn read_custom_nbt(&self, nbt: &pumpkin_nbt::compound::NbtCompound) {
        self.set_item_stack(super::read_thrown_item(
            self.get_entity(),
            nbt,
            &pumpkin_data::item::Item::SPLASH_POTION,
            pumpkin_data::tracked_data::splash_potion::ITEM_STACK,
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

    #[allow(clippy::too_many_lines)]
    fn on_hit(&self, hit: crate::entity::projectile::ProjectileHit) {
        let world = self.get_entity().world.load();
        let hit_pos = hit.hit_pos();

        let stack = self
            .item_stack
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        douse_fire_on_block_hit(&world, self, &hit, &stack);

        // Send impact entity status (plays break particles)
        world.send_entity_status(
            self.get_entity(),
            pumpkin_data::entity::EntityStatus::Death,
            pumpkin_protocol::bedrock::server::actor_event::ActorEventID::Death,
        );

        let effects = crate::item::potion::PotionContents::read_potion_effects(&stack);

        // Calculate color: custom_color if present, else blend of effects, else default water color
        let mut color = 0x385dc6; // default water-like color
        if let Some(pc) =
            stack.get_data_component::<pumpkin_data::data_component_impl::PotionContentsImpl>()
        {
            if let Some(c) = pc.custom_color {
                color = c;
            } else if !effects.is_empty() {
                let mut r_sum = 0.0;
                let mut g_sum = 0.0;
                let mut b_sum = 0.0;
                let count = effects.len() as f32;
                for (eff, _, _, _, _, _) in &effects {
                    let c = eff.color;
                    r_sum += ((c >> 16) & 0xFF) as f32;
                    g_sum += ((c >> 8) & 0xFF) as f32;
                    b_sum += (c & 0xFF) as f32;
                }
                let r = (r_sum / count) as i32;
                let g = (g_sum / count) as i32;
                let b = (b_sum / count) as i32;
                color = (r << 16) | (g << 8) | b;
            }
        } else if !effects.is_empty() {
            let mut r_sum = 0.0;
            let mut g_sum = 0.0;
            let mut b_sum = 0.0;
            let count = effects.len() as f32;
            for (eff, _, _, _, _, _) in &effects {
                let c = eff.color;
                r_sum += ((c >> 16) & 0xFF) as f32;
                g_sum += ((c >> 8) & 0xFF) as f32;
                b_sum += (c & 0xFF) as f32;
            }
            let r = (r_sum / count) as i32;
            let g = (g_sum / count) as i32;
            let b = (b_sum / count) as i32;
            color = (r << 16) | (g << 8) | b;
        }

        // Play splash particles
        let has_instant = effects.iter().any(|(e, _, _, _, _, _)| {
            e.id == pumpkin_data::effect::StatusEffect::INSTANT_DAMAGE.id
                || e.id == pumpkin_data::effect::StatusEffect::INSTANT_HEALTH.id
        });
        let event_id = if has_instant { 2007 } else { 2002 };

        // Convert hit_pos to BlockPos
        let block_pos = BlockPos(Vector3::new(
            hit_pos.x.floor() as i32,
            hit_pos.y.floor() as i32,
            hit_pos.z.floor() as i32,
        ));
        world.broadcast_to_chunk(
            to_chunk_pos(&Vector2::new(block_pos.0.x, block_pos.0.z)),
            &CWorldEvent::new(event_id, block_pos, color, false),
        );

        // A water bottle hurts water-sensitive mobs even though it has no effects.
        let hurts_water_sensitive = is_water_potion(&stack);

        // If no effects, just splash (like water bottles)
        if effects.is_empty() && !hurts_water_sensitive {
            return;
        }

        let radius = 4.0f64;
        let min = Vector3::new(hit_pos.x - radius, hit_pos.y - radius, hit_pos.z - radius);
        let max = Vector3::new(hit_pos.x + radius, hit_pos.y + radius, hit_pos.z + radius);
        let aabb = BoundingBox::new(min, max);

        // Gather entity and player candidates
        let candidates = world.get_entities_and_players_at_box(&aabb);

        let mut affected: Vec<(Arc<dyn EntityBase>, f32)> = Vec::new();
        for cand in candidates {
            if cand.get_living_entity().is_some() {
                let pos = cand.get_entity().pos.load();
                let dx = pos.x - hit_pos.x;
                let dy = pos.y - hit_pos.y;
                let dz = pos.z - hit_pos.z;
                let dist = (dx * dx + dy * dy + dz * dz).sqrt();
                if dist <= radius {
                    let scale = (1.0f32 - (dist as f32 / radius as f32)).max(0.0);
                    affected.push((cand, scale));
                }
            }
        }

        if let Some(server) = world.server.upgrade() {
            let affected_ids: Vec<i32> = affected
                .iter()
                .map(|(c, _)| c.get_entity().entity_id)
                .collect();
            let mut event =
                crate::plugin::api::events::entity::potion_splash::PotionSplashEvent::new(
                    self.get_entity().entity_id,
                    block_pos,
                    stack.item.registry_key.to_string(),
                    affected_ids,
                );
            server.plugin_manager.fire_blocking(&server, &mut event);
            if event.cancelled {
                return;
            }
            affected.retain(|(c, _)| event.affected_entities.contains(&c.get_entity().entity_id));
        }

        // Reuse the event-filtered entities so the event also gates water damage.
        if hurts_water_sensitive {
            let kept_ids: Vec<i32> = affected
                .iter()
                .map(|(c, _)| c.get_entity().entity_id)
                .collect();
            hurt_water_sensitive_entities(&world, self, hit_pos, Some(&kept_ids));
            rehydrate_axolotls(&world, self, hit_pos);
        }

        let owner = self.thrown.owner.get(&world);
        for (cand, scale) in affected {
            if let Some(living) = cand.get_living_entity() {
                crate::item::potion::PotionContents::apply_effects_to(
                    living,
                    effects.clone(),
                    scale,
                    crate::item::potion::PotionApplicationSource::Normal,
                    Some(crate::entity::living::EffectSource {
                        direct: self,
                        owner: owner.as_deref(),
                    }),
                );
            }
        }
    }
}
