use pumpkin_util::random::gameplay::game_random;
use std::sync::Arc;

use pumpkin_data::{
    Block, BlockState, BlockStateId,
    attributes::Attributes,
    damage::DamageType,
    enchantment::EnchantmentEntityEffect,
    entity::EntityType,
    fluid::Fluid,
    packet::CURRENT_MC_VERSION,
    particle::Particle,
    sound::Sound,
    tag::{RegistryKey, Tag, Taggable, get_registry_key_tags},
};
use pumpkin_util::math::{boundingbox::BoundingBox, position::BlockPos, vector3::Vector3};
use pumpkin_world::chunk::ChunkData;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::{
    block::{
        ExplodeArgs,
        blocks::fire::{FireBlockBase, fire::FireBlock},
        drop_loot,
    },
    entity::{Entity, EntityBase, combat::knockback_after_resistance},
    net::ClientPlatform,
    world::loot::LootContextParameters,
};

use super::{BlockFlags, World};

/// Defines the type of explosion interaction with the world.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExplosionInteraction {
    None,
    Block,
    Mob,
    Tnt,
    Trigger,
}

/// Defines how an explosion interacts with blocks in the world.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockInteraction {
    /// Keeps blocks intact (no block damage, no drops).
    Keep,
    /// Destroys blocks and drops 100% of items without decay.
    Destroy,
    /// Destroys blocks and applies loot decay based on explosion radius.
    DestroyWithDecay,
    /// Triggers block effects without destroying them.
    TriggerBlock,
}

/// Defines how damage and block destruction are calculated for an explosion.
pub trait ExplosionDamageCalculator: Send + Sync {
    /// Returns the block's explosion resistance. If None, the block is treated as air/empty.
    fn get_block_explosion_resistance(
        &self,
        _explosion: &Explosion,
        _world: &World,
        _pos: &BlockPos,
        block: &Block,
        fluid: &pumpkin_data::fluid::FluidState,
    ) -> Option<f32> {
        if block.default_state.is_air() && fluid.is_empty {
            None
        } else {
            Some(fluid.blast_resistance.max(block.blast_resistance))
        }
    }

    /// Returns whether this block should be destroyed / affected by the explosion.
    fn should_block_explode(
        &self,
        _explosion: &Explosion,
        _world: &World,
        _pos: &BlockPos,
        _block: &Block,
        _power: f32,
    ) -> bool {
        true
    }

    /// Returns whether the entity should take damage from the explosion.
    fn should_damage_entity(&self, _explosion: &Explosion, _entity: &dyn EntityBase) -> bool {
        true
    }

    /// Returns knockback multiplier for the given entity (default 1.0).
    fn get_knockback_multiplier(&self, _entity: &dyn EntityBase) -> f32 {
        1.0
    }

    /// Calculates the damage amount to deal to the entity given the exposure.
    fn get_entity_damage_amount(
        &self,
        explosion: &Explosion,
        entity: &dyn EntityBase,
        exposure: f32,
    ) -> f32 {
        let radius = explosion.power as f64 * 2.0;
        let distance = (entity
            .get_entity()
            .pos
            .load()
            .squared_distance_to_vec(&explosion.pos))
        .sqrt()
            / radius;
        let damage_multiplier = (1.0 - distance) * exposure as f64;
        (f64::midpoint(damage_multiplier * damage_multiplier, damage_multiplier)
            * 7.0
            * explosion.power as f64
            + 1.0) as f32
    }
}

/// Default explosion damage calculator implementing vanilla standard explosion rules.
pub struct DefaultExplosionDamageCalculator;

impl ExplosionDamageCalculator for DefaultExplosionDamageCalculator {}

/// A configurable explosion damage calculator (e.g. for wind charges, mace wind bursts).
pub struct SimpleExplosionDamageCalculator {
    pub damages_entities: bool,
    pub damages_blocks: bool,
    pub knockback_multiplier: Option<f32>,
    pub immune_blocks: Option<&'static Tag>,
}

impl SimpleExplosionDamageCalculator {
    #[must_use]
    pub const fn new(
        damages_entities: bool,
        damages_blocks: bool,
        knockback_multiplier: Option<f32>,
        immune_blocks: Option<&'static Tag>,
    ) -> Self {
        Self {
            damages_entities,
            damages_blocks,
            knockback_multiplier,
            immune_blocks,
        }
    }
}

impl ExplosionDamageCalculator for SimpleExplosionDamageCalculator {
    fn get_block_explosion_resistance(
        &self,
        _explosion: &Explosion,
        _world: &World,
        _pos: &BlockPos,
        block: &Block,
        fluid: &pumpkin_data::fluid::FluidState,
    ) -> Option<f32> {
        if let Some(immune_tag) = self.immune_blocks
            && block.has_tag(immune_tag)
        {
            return None;
        }
        if block.default_state.is_air() && fluid.is_empty {
            None
        } else {
            Some(fluid.blast_resistance.max(block.blast_resistance))
        }
    }

    fn should_block_explode(
        &self,
        _explosion: &Explosion,
        _world: &World,
        _pos: &BlockPos,
        block: &Block,
        _power: f32,
    ) -> bool {
        if !self.damages_blocks {
            return false;
        }
        if let Some(immune_tag) = self.immune_blocks
            && block.has_tag(immune_tag)
        {
            return false;
        }
        true
    }

    fn should_damage_entity(&self, _explosion: &Explosion, _entity: &dyn EntityBase) -> bool {
        self.damages_entities
    }

    fn get_knockback_multiplier(&self, _entity: &dyn EntityBase) -> f32 {
        self.knockback_multiplier.unwrap_or(1.0)
    }
}

/// Vanilla `Explosion.getIndirectSourceEntity`'s result, which may be the source itself.
enum IndirectSource<'a> {
    None,
    Source(&'a dyn EntityBase),
    Entity(Arc<dyn EntityBase>),
}

pub struct Explosion<'a> {
    /// Vanilla `ServerExplosion.source`: the exploding entity, which the blast skips.
    source: Option<&'a dyn EntityBase>,
    indirect_source: IndirectSource<'a>,
    /// The causing entity of an explicit `DamageSources.explosion(source, attacker)`.
    attacker: Option<&'a dyn EntityBase>,
    power: f32,
    pos: Vector3<f64>,
    block_interaction: BlockInteraction,
    damage_calculator: Option<Arc<dyn ExplosionDamageCalculator>>,
    preserve_rails: bool,
    /// Vanilla `ServerExplosion.fire`: sets fire to some of the blown positions.
    create_fire: bool,
    pub(super) small_particle: Particle,
    pub(super) large_particle: Particle,
    pub(super) sound: Sound,
    /// A damage source without entities, set in place of `DamageSources.explosion`.
    damage_type: Option<DamageType>,
}

impl<'a> Explosion<'a> {
    #[must_use]
    pub const fn new(power: f32, pos: Vector3<f64>, block_interaction: BlockInteraction) -> Self {
        Self {
            source: None,
            indirect_source: IndirectSource::None,
            attacker: None,
            power,
            pos,
            block_interaction,
            damage_calculator: None,
            preserve_rails: false,
            create_fire: false,
            small_particle: Particle::Explosion,
            large_particle: Particle::ExplosionEmitter,
            sound: Sound::EntityGenericExplode,
            damage_type: None,
        }
    }

    /// Vanilla `ServerExplosion.source`, which also makes the default damage source
    /// `DamageSources.explosion(source, getIndirectSourceEntity())`.
    #[must_use]
    pub fn with_source(mut self, source: Option<&'a dyn EntityBase>) -> Self {
        self.source = source;
        self.indirect_source = source.map_or(IndirectSource::None, Self::indirect_source_of);
        self
    }

    /// An explicit `DamageSources.explosion(source, attacker)`, as end crystals pass.
    /// `None` keeps the default damage source, like vanilla's `null`.
    #[must_use]
    pub fn with_attacker(mut self, attacker: Option<&'a dyn EntityBase>) -> Self {
        self.attacker = attacker;
        self
    }

    /// Vanilla `Explosion.getIndirectSourceEntity`: a primed TNT's owner, a living source
    /// itself, or a projectile's living owner.
    fn indirect_source_of(source: &'a dyn EntityBase) -> IndirectSource<'a> {
        if let Some(tnt) = source
            .cast_any()
            .downcast_ref::<crate::entity::tnt::TNTEntity>()
        {
            return tnt
                .owner()
                .map_or(IndirectSource::None, IndirectSource::Entity);
        }
        if source.get_living_entity().is_some() {
            return IndirectSource::Source(source);
        }
        source
            .get_owner_entity()
            .filter(|owner| owner.get_living_entity().is_some())
            .map_or(IndirectSource::None, IndirectSource::Entity)
    }

    #[must_use]
    pub fn source_id(&self) -> Option<i32> {
        self.source.map(|source| source.get_entity().entity_id)
    }

    /// Vanilla `getIndirectSourceEntity`.
    #[must_use]
    pub fn indirect_source_entity(&self) -> Option<&dyn EntityBase> {
        match &self.indirect_source {
            IndirectSource::None => None,
            IndirectSource::Source(source) => Some(*source),
            IndirectSource::Entity(entity) => Some(entity.as_ref()),
        }
    }

    /// The damage source's type, direct entity and causing entity.
    fn damage_source(&self) -> (DamageType, Option<&dyn EntityBase>, Option<&dyn EntityBase>) {
        if let Some(damage_type) = self.damage_type {
            return (damage_type, None, None);
        }
        let attacker = self.attacker.or_else(|| self.indirect_source_entity());
        // Vanilla `DamageSources.explosion(entity, cause)`.
        let damage_type = if self.source.is_some() && attacker.is_some() {
            DamageType::PLAYER_EXPLOSION
        } else {
            DamageType::EXPLOSION
        };
        (damage_type, self.source, attacker)
    }

    /// Hurts entities with `damage_type` instead of a plain explosion.
    #[must_use]
    pub const fn with_damage_type(mut self, damage_type: DamageType) -> Self {
        self.damage_type = Some(damage_type);
        self
    }

    /// The particles and sound vanilla's `ServerLevel.explode` sends with the explosion.
    #[must_use]
    pub const fn with_particles_and_sound(
        mut self,
        small_particle: Particle,
        large_particle: Particle,
        sound: Sound,
    ) -> Self {
        self.small_particle = small_particle;
        self.large_particle = large_particle;
        self.sound = sound;
        self
    }

    /// Vanilla `ServerExplosion.isSmall`.
    pub(super) fn is_small(&self) -> bool {
        self.power < 2.0 || self.block_interaction == BlockInteraction::Keep
    }

    #[must_use]
    pub fn with_damage_calculator(
        mut self,
        calculator: Arc<dyn ExplosionDamageCalculator>,
    ) -> Self {
        self.damage_calculator = Some(calculator);
        self
    }

    #[must_use]
    pub const fn preserving_rails(mut self) -> Self {
        self.preserve_rails = true;
        self
    }

    #[must_use]
    pub const fn with_fire(mut self) -> Self {
        self.create_fire = true;
        self
    }

    fn protects_rail(&self, world: &World, pos: &BlockPos, block: &Block) -> bool {
        self.preserve_rails && (Self::is_rail(block) || Self::is_rail(world.get_block(&pos.up())))
    }

    fn is_rail(block: &Block) -> bool {
        block.id == Block::RAIL.id
            || block.id == Block::POWERED_RAIL.id
            || block.id == Block::DETECTOR_RAIL.id
            || block.id == Block::ACTIVATOR_RAIL.id
    }

    #[allow(clippy::too_many_lines)]
    /// Also returns every position the rays blew through, air included, which vanilla's
    /// `createFire` picks from; only collected for explosions that make fire.
    fn get_blocks_to_destroy(
        &self,
        world: &World,
    ) -> (
        FxHashMap<BlockPos, (&'static Block, &'static BlockState)>,
        FxHashSet<BlockPos>,
    ) {
        let mut map = FxHashMap::default();
        let mut blown = FxHashSet::default();

        let mut chunk_cache: FxHashMap<
            pumpkin_util::math::vector2::Vector2<i32>,
            Option<Arc<ChunkData>>,
        > = FxHashMap::default();

        let default_calc = DefaultExplosionDamageCalculator;
        let calc: &dyn ExplosionDamageCalculator = match &self.damage_calculator {
            Some(c) => c.as_ref(),
            None => &default_calc,
        };

        for x in 0..16 {
            for y in 0..16 {
                for z in 0..16 {
                    if x > 0 && x < 15 && y > 0 && y < 15 && z > 0 && z < 15 {
                        continue;
                    }

                    let mut dir_x = f64::from(x) / 7.5 - 1.0;
                    let mut dir_y = f64::from(y) / 7.5 - 1.0;
                    let mut dir_z = f64::from(z) / 7.5 - 1.0;

                    let length = (dir_x * dir_x + dir_y * dir_y + dir_z * dir_z).sqrt();
                    dir_x /= length;
                    dir_y /= length;
                    dir_z /= length;

                    let mut pos_x = self.pos.x;
                    let mut pos_y = self.pos.y;
                    let mut pos_z = self.pos.z;

                    let random_val = game_random().random::<f32>();
                    let mut h = self.power * random_val.mul_add(0.6, 0.7);

                    while h > 0.0 {
                        let block_pos = BlockPos::floored(pos_x, pos_y, pos_z);

                        if !world.is_in_build_limit(block_pos) {
                            break;
                        }

                        let (chunk_pos, relative) = block_pos.chunk_and_chunk_relative_position();

                        let chunk_opt = chunk_cache.entry(chunk_pos).or_insert_with(|| {
                            world
                                .level
                                .read_chunk_sync(&chunk_pos, std::clone::Clone::clone)
                        });

                        let state_id = if let Some(chunk) = chunk_opt {
                            chunk
                                .section
                                .get_block_absolute_y(
                                    relative.x as usize,
                                    block_pos.0.y,
                                    relative.z as usize,
                                )
                                .unwrap_or(Block::AIR.default_state.id)
                        } else {
                            Block::AIR.default_state.id
                        };

                        let (block, state) = BlockState::from_id_with_block(state_id);

                        let (_fluid, fluid_state) = Fluid::from_state_id(state_id).map_or_else(
                            || {
                                if block.is_waterlogged(state_id) {
                                    (&Fluid::FLOWING_WATER, &Fluid::FLOWING_WATER.states[0])
                                } else {
                                    (&Fluid::EMPTY, &Fluid::EMPTY.states[0])
                                }
                            },
                            |raw_fluid| {
                                let f = raw_fluid.to_flowing();
                                (f, &f.states[0])
                            },
                        );

                        if !state.is_air() || !fluid_state.is_empty {
                            let protects_rail = self.protects_rail(world, &block_pos, block);
                            let resistance = if protects_rail {
                                Some(0.0)
                            } else {
                                calc.get_block_explosion_resistance(
                                    self,
                                    world,
                                    &block_pos,
                                    block,
                                    fluid_state,
                                )
                            };

                            if let Some(resistance) = resistance {
                                h -= (resistance + 0.3) * 0.3;
                            }

                            if h > 0.0
                                && !protects_rail
                                && calc.should_block_explode(self, world, &block_pos, block, h)
                            {
                                map.insert(block_pos, (block, state));
                                if self.create_fire {
                                    blown.insert(block_pos);
                                }
                            }
                        } else if self.create_fire
                            && h > 0.0
                            && calc.should_block_explode(self, world, &block_pos, block, h)
                        {
                            blown.insert(block_pos);
                        }

                        pos_x += dir_x * 0.3;
                        pos_y += dir_y * 0.3;
                        pos_z += dir_z * 0.3;
                        h -= 0.225_000_01;
                    }
                }
            }
        }
        (map, blown)
    }

    /// Vanilla `ServerExplosion.createFire`.
    fn create_fire(world: &Arc<World>, blown: &FxHashSet<BlockPos>) {
        for pos in blown {
            if game_random().random_range(0..3) == 0
                && world.get_block_state(pos).is_air()
                && world.get_block_state(&pos.down()).is_solid_render()
            {
                let fire_block = FireBlockBase::get_fire_type(world, pos);
                let state_id = if fire_block == Block::FIRE {
                    FireBlock.get_state_for_position(world, &fire_block, pos)
                } else {
                    fire_block.default_state.id
                };
                world.set_block_state(pos, state_id, BlockFlags::NOTIFY_ALL);
            }
        }
    }

    /// Returns the knockback for each hit Java player, which vanilla sends in the explosion
    /// packet for the client to add to its own motion.
    fn damage_entities(&self, world: &Arc<World>) -> FxHashMap<i32, Vector3<f64>> {
        let mut hit_players = FxHashMap::default();
        // Explosion is too small
        if self.power < 1.0e-5 {
            return hit_players;
        }

        let radius = self.power as f64 * 2.0;
        let min_x = (self.pos.x - radius - 1.0).floor() as i32;
        let max_x = (self.pos.x + radius + 1.0).floor() as i32;
        let min_y = (self.pos.y - radius - 1.0).floor() as i32;
        let max_y = (self.pos.y + radius + 1.0).floor() as i32;
        let min_z = (self.pos.z - radius - 1.0).floor() as i32;
        let max_z = (self.pos.z + radius + 1.0).floor() as i32;

        let search_box = BoundingBox::new(
            Vector3::new(min_x as f64, min_y as f64, min_z as f64),
            Vector3::new(max_x as f64, max_y as f64, max_z as f64),
        );

        let entities = world.get_all_at_box(&search_box);

        let default_calc = DefaultExplosionDamageCalculator;
        let calc: &dyn ExplosionDamageCalculator = match &self.damage_calculator {
            Some(c) => c.as_ref(),
            None => &default_calc,
        };

        let (damage_type, direct, attacker) = self.damage_source();
        let source_id = self.source_id();
        for entity_base in entities {
            // Vanilla `level.getEntities(source, ...)` leaves the source out.
            if source_id == Some(entity_base.get_entity().entity_id) {
                continue;
            }
            if entity_base.is_immune_to_explosion() {
                continue;
            }

            // Skip spectators (no damage, no knockback)
            if entity_base.is_spectator() {
                continue;
            }

            let entity = entity_base.get_entity();

            let distance = (entity.pos.load().squared_distance_to_vec(&self.pos)).sqrt() / radius;
            if distance > 1.0 {
                continue;
            }

            let should_damage = calc.should_damage_entity(self, entity_base.as_ref());
            let knockback_multiplier = calc.get_knockback_multiplier(entity_base.as_ref()) as f64;

            let exposure = if !should_damage && knockback_multiplier == 0.0 {
                0.0
            } else {
                Self::calculate_exposure(&self.pos, entity, world) as f64
            };

            if exposure == 0.0 {
                continue;
            }

            if should_damage {
                let damage =
                    calc.get_entity_damage_amount(self, entity_base.as_ref(), exposure as f32);
                entity_base.damage_with_context(
                    entity_base.as_ref(),
                    damage,
                    damage_type,
                    None,
                    direct,
                    attacker,
                );
            }

            // Calculate and apply knockback
            let dir_pos = if entity.entity_type == &EntityType::TNT {
                entity.pos.load()
            } else {
                entity.get_eye_pos()
            };
            let direction = (dir_pos - self.pos).normalize();

            let knockback_resistance = entity_base.get_living_entity().map_or(0.0, |living| {
                living.get_attribute_value(&Attributes::EXPLOSION_KNOCKBACK_RESISTANCE)
            });
            let knockback_power = knockback_after_resistance(
                (1.0 - distance) * exposure * knockback_multiplier,
                knockback_resistance,
            );
            let knockback = direction * knockback_power;
            // Vanilla `ServerExplosion.hurtEntities`: creative flyers get no knockback.
            if entity_base
                .get_player()
                .is_some_and(|player| player.is_creative() && player.is_flying())
            {
                continue;
            }
            if entity_base
                .get_player()
                .is_some_and(|player| matches!(player.client.as_ref(), ClientPlatform::Java(_)))
            {
                entity.velocity.store(entity.velocity.load() + knockback);
                hit_players.insert(entity.entity_id, knockback);
            } else {
                entity.add_velocity(knockback);
            }
        }
        hit_players
    }

    fn calculate_exposure(
        explosion_pos: &Vector3<f64>,
        entity: &Entity,
        world: &Arc<World>,
    ) -> f32 {
        let bbox = entity.bounding_box.load();

        let step_x = 1.0 / ((bbox.max.x - bbox.min.x) * 2.0 + 1.0);
        let step_y = 1.0 / ((bbox.max.y - bbox.min.y) * 2.0 + 1.0);
        let step_z = 1.0 / ((bbox.max.z - bbox.min.z) * 2.0 + 1.0);

        if step_x < 0.0 || step_y < 0.0 || step_z < 0.0 {
            return 0.0;
        }

        let offset_x = (1.0 - (1.0 / step_x).floor() * step_x) / 2.0;
        let offset_z = (1.0 - (1.0 / step_z).floor() * step_z) / 2.0;

        let mut visible_points = 0;
        let mut total_points = 0;

        let mut k = 0.0;
        while k <= 1.0 {
            let mut l = 0.0;
            while l <= 1.0 {
                let mut m = 0.0;
                while m <= 1.0 {
                    let n = bbox.min.x + (bbox.max.x - bbox.min.x) * k;
                    let o = bbox.min.y + (bbox.max.y - bbox.min.y) * l;
                    let p = bbox.min.z + (bbox.max.z - bbox.min.z) * m;

                    let vec3d = Vector3::new(n + offset_x, o, p + offset_z);

                    if world
                        .clip(&pumpkin_world::clip::ClipContext::new(
                            vec3d,
                            *explosion_pos,
                            pumpkin_world::clip::ClipBlock::Collider,
                            pumpkin_world::clip::ClipFluid::None,
                        ))
                        .is_miss
                    {
                        visible_points += 1;
                    }

                    total_points += 1;
                    m += step_z;
                }
                l += step_y;
            }
            k += step_x;
        }

        if total_points == 0 {
            return 0.0;
        }

        visible_points as f32 / total_points as f32
    }

    /// Returns the removed block count
    /// Returns the removed block count and the knockback of each hit Java player.
    pub fn explode(&self, world: &Arc<World>) -> (u32, FxHashMap<i32, Vector3<f64>>) {
        let hit_players = self.damage_entities(world);
        (self.interact_with_blocks(world), hit_players)
    }

    fn interact_with_blocks(&self, world: &Arc<World>) -> u32 {
        match self.block_interaction {
            BlockInteraction::Keep => {
                if self.create_fire {
                    let (_, blown) = self.get_blocks_to_destroy(world);
                    Self::create_fire(world, &blown);
                }
                0
            }
            BlockInteraction::TriggerBlock => {
                let (blocks, blown) = self.get_blocks_to_destroy(world);
                for (pos, (block, _state)) in &blocks {
                    let pumpkin_block = world.block_registry.get_pumpkin_block(block.id);
                    if let Some(pumpkin_block) = pumpkin_block {
                        pumpkin_block.explode(ExplodeArgs {
                            world,
                            block,
                            position: pos,
                            indirect_source: self.indirect_source_entity(),
                        });
                    }
                }
                Self::create_fire(world, &blown);
                0
            }
            BlockInteraction::Destroy | BlockInteraction::DestroyWithDecay => {
                let center_pos = BlockPos::floored(self.pos.x, self.pos.y, self.pos.z);
                let mut event =
                    crate::plugin::api::events::block::block_explode::BlockExplodeEvent::new(
                        center_pos,
                        if self.power > 0.0 {
                            1.0 / self.power
                        } else {
                            1.0
                        },
                    );
                if let Some(server) = world.server.upgrade() {
                    server.plugin_manager.fire_blocking(&server, &mut event);
                }
                if event.cancelled {
                    return 0;
                }

                let (blocks, blown) = self.get_blocks_to_destroy(world);
                let decay_drops = self.block_interaction == BlockInteraction::DestroyWithDecay;
                let explosion_radius = decay_drops.then_some(self.power);

                for (pos, (block, state)) in &blocks {
                    let pumpkin_block = world.block_registry.get_pumpkin_block(block.id);

                    if pumpkin_block.is_none_or(|s| s.should_drop_items_on_explosion()) {
                        let is_raining = world.is_raining();
                        let is_thundering = world.is_thundering();
                        let params = LootContextParameters {
                            block_state: Some(state),
                            explosion_radius,
                            position: Some(pumpkin_util::math::vector3::Vector3::new(
                                pos.0.x as f64,
                                pos.0.y as f64,
                                pos.0.z as f64,
                            )),
                            world_time: world.level_info.load().day_time as u64,
                            is_raining: Some(is_raining),
                            is_thundering: Some(is_thundering),
                            ..Default::default()
                        };
                        drop_loot(world, block, pos, false, &params);
                    }

                    world.set_block_state(pos, BlockStateId::AIR, BlockFlags::NOTIFY_ALL);
                    world.close_container_screens_at(pos);

                    if let Some(pumpkin_block) = pumpkin_block {
                        pumpkin_block.explode(ExplodeArgs {
                            world,
                            block,
                            position: pos,
                            indirect_source: self.indirect_source_entity(),
                        });
                    }
                }
                Self::create_fire(world, &blown);
                blocks.len() as u32
            }
        }
    }
}

/// Vanilla `ExplodeEffect.apply` for an enchantment's `minecraft:explode` effect.
pub fn apply_explode_effect(
    world: &Arc<World>,
    effect: &EnchantmentEntityEffect,
    level: i32,
    position: Vector3<f64>,
) {
    let EnchantmentEntityEffect::Explode {
        damage_type,
        knockback_multiplier,
        immune_blocks,
        offset_x,
        offset_y,
        offset_z,
        radius,
        create_fire,
        block_interaction,
        small_particle,
        large_particle,
        sound,
        ..
    } = effect
    else {
        return;
    };
    let interaction = match *block_interaction {
        "block" => ExplosionInteraction::Block,
        "mob" => ExplosionInteraction::Mob,
        "tnt" => ExplosionInteraction::Tnt,
        "trigger" => ExplosionInteraction::Trigger,
        _ => ExplosionInteraction::None,
    };
    let immune_blocks = immune_blocks.and_then(|tag| {
        get_registry_key_tags(CURRENT_MC_VERSION, RegistryKey::Block)?
            .get(tag.strip_prefix('#').unwrap_or(tag))
            .copied()
    });
    let calculator = SimpleExplosionDamageCalculator::new(
        damage_type.is_some(),
        interaction != ExplosionInteraction::None,
        knockback_multiplier
            .as_ref()
            .map(|value| value.calculate(level)),
        immune_blocks,
    );
    let pos = position + Vector3::new(*offset_x, *offset_y, *offset_z);
    let radius = radius.calculate(level).max(0.0);
    let mut explosion = Explosion::new(radius, pos, world.get_block_interaction(interaction))
        .with_damage_calculator(Arc::new(calculator))
        .with_particles_and_sound(
            small_particle.unwrap_or(Particle::Explosion),
            large_particle.unwrap_or(Particle::ExplosionEmitter),
            sound.unwrap_or(Sound::EntityGenericExplode),
        );
    if *create_fire {
        explosion = explosion.with_fire();
    }
    world.run_explosion(&explosion, pos, radius);
}

#[cfg(test)]
mod tests {
    use super::Explosion;
    use pumpkin_data::Block;

    #[test]
    fn tnt_minecart_rail_protection_covers_every_rail_type() {
        for rail in [
            &Block::RAIL,
            &Block::POWERED_RAIL,
            &Block::DETECTOR_RAIL,
            &Block::ACTIVATOR_RAIL,
        ] {
            assert!(Explosion::is_rail(rail));
        }
        assert!(!Explosion::is_rail(&Block::STONE));
    }
}
