use pumpkin_data::noise_settings::NoiseSettings;
use pumpkin_data::placed_feature::PlacedFeature as PlacedFeatureKey;
use pumpkin_data::structures::{Structure, StructureKeys, StructureType};
use pumpkin_data::translation;
use pumpkin_data::{Block, BlockStateId, Mirror, Rotation};
use pumpkin_util::identifier::Identifier;
use pumpkin_util::math::block_box::BlockBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::random::hash_block_pos;
use pumpkin_util::text::TextComponent;
use pumpkin_world::generation::proto_chunk::ProtoChunk;
use pumpkin_world::world::{BlockAccessor, WorldPortalExt};

use crate::command::argument_builder::{
    ArgumentBuilder, LiteralArgumentBuilder, argument, command, literal,
};
use crate::command::argument_types::argument_type::JavaClientArgumentType;
use crate::command::argument_types::coordinates::block_pos::BlockPosArgumentType;
use crate::command::argument_types::core::float::FloatArgumentType;
use crate::command::argument_types::core::integer::IntegerArgumentType;
use crate::command::argument_types::identifier::IdentifierArgumentType;
use crate::command::argument_types::registry_id::RegistryIdArgumentType;
use crate::command::argument_types::template::TemplateNameArgumentType;
use crate::command::argument_types::template_transform::{
    TemplateMirrorArgumentType, TemplateRotationArgumentType,
};
use crate::command::context::command_context::CommandContext;
use crate::command::errors::error_types::CommandErrorType;
use crate::command::node::dispatcher::CommandDispatcher;
use crate::command::node::{CommandExecutor, CommandExecutorResult};
use crate::world::World;
use crate::world::block_placer::WorldBlockPlacer;
use pumpkin_command::argument_types::coordinates::block_pos::NOT_LOADED_ERROR_TYPE;
use pumpkin_command::errors::command_syntax_error::CommandSyntaxError;
use pumpkin_util::math::vector2::Vector2;
use pumpkin_util::random::gameplay::game_random;
use pumpkin_world::generation::feature::configured_features::CONFIGURED_FEATURES;
use pumpkin_world::generation::feature::placed_features::{Feature, PLACED_FEATURES};
use pumpkin_world::generation::structure::structures::jigsaw::{
    PoolElementStructurePiece, place_pool_element_templates,
};
use pumpkin_world::generation::structure::structures::jigsaw_placement::{
    DimensionPadding, JigsawPlacement, LiquidSettings, MaxDistance, PoolAliasLookup,
};
use pumpkin_world::generation::structure::structures::{
    StructureGeneratorContext, start_reference_pos,
};
use pumpkin_world::generation::structure::template::BlockPlacer;
use pumpkin_world::generation::structure::template::{StructurePlaceSettings, StructureProcessor};
use rustc_hash::FxHashSet;
use std::sync::Arc;

use pumpkin_util::PermissionLvl;
use pumpkin_util::permission::{Permission, PermissionDefault, PermissionRegistry};
use pumpkin_util::random::RandomGenerator;
use pumpkin_util::random::legacy_rand::LegacyRand;

const DESCRIPTION: &str = "Places a structure template in the world.";
const PERMISSION: &str = "minecraft:command.place";

static TEMPLATE_NOT_FOUND: CommandErrorType<1> = CommandErrorType::new(
    translation::java::COMMANDS_PLACE_TEMPLATE_INVALID,
    "commands.place.template.invalid",
);
static TEMPLATE_FAILED: CommandErrorType<0> = CommandErrorType::new(
    translation::java::COMMANDS_PLACE_TEMPLATE_FAILED,
    "commands.place.template.failed",
);
static JIGSAW_FAILED: CommandErrorType<1> = CommandErrorType::new(
    translation::java::COMMANDS_PLACE_JIGSAW_FAILED,
    "commands.place.jigsaw.failed",
);
static STRUCTURE_INVALID: CommandErrorType<1> = CommandErrorType::new(
    translation::java::COMMANDS_PLACE_STRUCTURE_INVALID,
    "commands.place.structure.invalid",
);
static STRUCTURE_FAILED: CommandErrorType<0> = CommandErrorType::new(
    translation::java::COMMANDS_PLACE_STRUCTURE_FAILED,
    "commands.place.structure.failed",
);
/// Vanilla 26.3 reports an unknown feature through its `ResourceOrIdArgument`; the old
/// `commands.place.feature.invalid` key is gone from the client.
static FEATURE_INVALID: CommandErrorType<2> = CommandErrorType::new(
    translation::java::ARGUMENT_RESOURCE_OR_ID_NO_SUCH_ELEMENT,
    "argument.resource_or_id.no_such_element",
);
const FEATURE_REGISTRY: &str = "minecraft:worldgen/feature";

const CHUNK_DIM: i32 = 16;

/// Clamps a world Y into valid chunk bounds, placing the surface one block below the target.
fn ground_y(block_y: i32, chunk_min_y: i32, chunk_height: i32) -> i32 {
    (block_y - 1).clamp(chunk_min_y, chunk_min_y + chunk_height - 1)
}

/// Vanilla-style chunk-seed derivation used by structure pieces and feature generators.
const fn chunk_population_seed(cx: i32, cz: i32, world_seed: u64) -> u64 {
    (cx as i64)
        .wrapping_mul(341873128712)
        .wrapping_add((cz as i64).wrapping_mul(132897987541))
        .wrapping_add(world_seed as i64) as u64
}

/// Minimal `WorldPortalExt` implementation for synthetic chunk placement.
struct CommandBlockRegistry;

impl WorldPortalExt for CommandBlockRegistry {
    fn can_place_at(
        &self,
        block: &pumpkin_data::Block,
        state: &pumpkin_data::BlockState,
        block_accessor: &dyn BlockAccessor,
        block_pos: &BlockPos,
    ) -> bool {
        crate::block::registry::default_registry().can_place_at(
            None,
            None,
            block_accessor,
            None,
            block,
            state,
            block_pos,
            None,
            None,
        )
    }

    fn mirror(
        &self,
        block: &pumpkin_data::Block,
        state_id: BlockStateId,
        mirror: Mirror,
    ) -> &'static pumpkin_data::BlockState {
        block.mirror(state_id, mirror)
    }

    fn rotate(
        &self,
        block: &pumpkin_data::Block,
        state_id: BlockStateId,
        rotation: Rotation,
    ) -> &'static pumpkin_data::BlockState {
        block.rotate(state_id, rotation)
    }

    fn spawn_mobs_for_chunk_generation(
        &self,
        _cache: &mut dyn pumpkin_world::generation::proto_chunk::GenerationCache,
        _biome: &'static pumpkin_data::biome::Biome,
        _chunk_x: i32,
        _chunk_z: i32,
    ) {
    }
}

/// `/place template`. `args` counts the optional arguments after the template name that the
/// command was given: pos, rotation, mirror, integrity and seed, in that order.
struct PlaceTemplateExecutor {
    args: u8,
    strict: bool,
}

impl CommandExecutor for PlaceTemplateExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let template_name = context.get_argument::<String>("template")?.clone();
        let block_pos = if self.args >= 1 {
            BlockPosArgumentType::get_loaded_block_pos(context, "pos")?
        } else {
            BlockPos::floored_v(context.source.position)
        };
        let rotation = if self.args >= 2 {
            TemplateRotationArgumentType::get(context, "rotation")?
        } else {
            Rotation::None
        };
        let mirror = if self.args >= 3 {
            TemplateMirrorArgumentType::get(context, "mirror")?
        } else {
            Mirror::None
        };
        let integrity = if self.args >= 4 {
            FloatArgumentType::get(context, "integrity")?
        } else {
            1.0
        };
        let seed = if self.args >= 5 {
            IntegerArgumentType::get(context, "seed")?
        } else {
            0
        };

        let Some(template) =
            pumpkin_world::generation::structure::template::get_template(&template_name)
        else {
            return Err(
                TEMPLATE_NOT_FOUND.create_without_context(TextComponent::text(template_name))
            );
        };
        let world = context.world();
        // Vanilla checks up to the position offset by the unrotated size.
        check_loaded(
            world,
            block_pos.chunk_position(),
            BlockPos(block_pos.0 + template.size).chunk_position(),
        )?;

        let mut settings = StructurePlaceSettings::new()
            .set_mirror(mirror)
            .set_rotation(rotation)
            .set_known_shape(self.strict);
        if integrity < 1.0 {
            settings = settings.add_processor(StructureProcessor::BlockRot {
                integrity,
                rottable_blocks: None,
            });
        }
        let mut settings_random = (integrity < 1.0).then(|| structure_block_random(seed));

        let mut placer = WorldBlockPlacer::new(world);
        let mut entities = Vec::new();
        let placed = pumpkin_world::generation::structure::template::place_in_world(
            &mut placer,
            &template,
            block_pos.0,
            &settings,
            settings_random.as_mut(),
            &mut structure_block_random(seed),
            |entity| entities.push(entity),
        );
        if !placed {
            return Err(TEMPLATE_FAILED.create_without_context());
        }
        placer.finalize();
        world.queue_block_updates(&placer.changed_positions);
        if !self.strict {
            // `updateNeighborsAt` for every placed block, which strict placement skips.
            let mut updated = FxHashSet::default();
            for (pos, _) in &placer.changed_positions {
                if updated.insert(*pos) {
                    world.update_neighbors_at(pos, world.get_block(pos), None);
                }
            }
        }
        world.spawn_structure_entities(entities);

        context.source.send_feedback(
            pumpkin_macros::translate_cross!(
                translation::java::COMMANDS_PLACE_TEMPLATE_SUCCESS,
                translation::bedrock::COMMANDS_PLACE_SUCCESS,
                TextComponent::text(template_name),
                TextComponent::text(block_pos.0.x.to_string()),
                TextComponent::text(block_pos.0.y.to_string()),
                TextComponent::text(block_pos.0.z.to_string())
            ),
            true,
        );

        Ok(1)
    }
}

/// Vanilla `StructureBlockEntity.createRandom`: a seed of 0 means a random one.
fn structure_block_random(seed: i32) -> LegacyRand {
    let seed = if seed == 0 {
        game_random().next_i64()
    } else {
        i64::from(seed)
    };
    LegacyRand::from_seed(seed as u64)
}

/// Vanilla `PlaceCommand.checkLoaded`.
fn check_loaded(
    world: &Arc<World>,
    chunk_min: Vector2<i32>,
    chunk_max: Vector2<i32>,
) -> Result<(), CommandSyntaxError> {
    for x in chunk_min.x..=chunk_max.x {
        for z in chunk_min.y..=chunk_max.y {
            if world
                .level
                .read_chunk_sync(&Vector2::new(x, z), |_| ())
                .is_none()
            {
                return Err(NOT_LOADED_ERROR_TYPE.create_without_context());
            }
        }
    }
    Ok(())
}

struct PlaceJigsawExecutor;

impl CommandExecutor for PlaceJigsawExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let pool = context.get_argument::<Identifier>("pool")?.to_string();
        let target = context.get_argument::<Identifier>("target")?.to_string();
        let max_depth = *context.get_argument::<i32>("max_depth")?;

        let block_pos = BlockPosArgumentType::get_block_pos(context, "pos").unwrap_or_else(|_| {
            let p = context.source.position;
            BlockPos::new(p.x as i32, p.y as i32, p.z as i32)
        });

        let mut entities = Vec::new();
        let (_piece_count, placer) = {
            let seed = hash_block_pos(block_pos.0.x, block_pos.0.y, block_pos.0.z) as u64;
            let random = RandomGenerator::Legacy(LegacyRand::from_seed(seed));
            let world_gen = context.world().level.world_gen();
            let settings = NoiseSettings::from_dimension(world_gen.dimension());
            let mut structure_context = StructureGeneratorContext {
                seed: seed as i64,
                chunk_x: 0,
                chunk_z: 0,
                random,
                sea_level: settings.sea_level,
                min_y: (settings.shape.min_y as i32).max(world_gen.dimension().min_y),
                height: settings
                    .shape
                    .height
                    .min(world_gen.dimension().height as u16),
                height_sampler: None,
                structure_key: None,
            };

            let position = JigsawPlacement::add_pieces(
                &mut structure_context,
                &pool,
                Some(&target),
                max_depth,
                block_pos,
                false,
                false,
                &MaxDistance::new(128),
                DimensionPadding::ZERO,
                LiquidSettings::ApplyWaterlog,
                &PoolAliasLookup::default(),
            )
            .ok_or_else(|| {
                JIGSAW_FAILED.create_without_context(TextComponent::text(pool.clone()))
            })?;

            let collector = position
                .collector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let piece_count = collector.pieces.len();

            let mut placer = WorldBlockPlacer::new(context.world());
            for piece in &collector.pieces {
                if let Some(jigsaw_piece) =
                    piece.as_any().downcast_ref::<PoolElementStructurePiece>()
                {
                    // Vanilla `JigsawPlacement.generateJigsaw` places with the command position.
                    place_pool_element_templates(
                        jigsaw_piece,
                        &mut placer,
                        None,
                        false,
                        block_pos.0,
                        context.world().level.seed.0 as i64,
                        Some(&mut |entity| entities.push(entity)),
                    );
                }
            }

            (piece_count, placer)
        };

        placer.finalize();
        context
            .world()
            .queue_block_updates(&placer.changed_positions);
        context.world().spawn_structure_entities(entities);

        context.source.send_feedback(
            pumpkin_macros::translate_cross!(
                translation::java::COMMANDS_PLACE_JIGSAW_SUCCESS,
                translation::bedrock::COMMANDS_PLACE_SUCCESS,
                TextComponent::text(block_pos.0.x.to_string()),
                TextComponent::text(block_pos.0.y.to_string()),
                TextComponent::text(block_pos.0.z.to_string())
            ),
            true,
        );

        Ok(1)
    }
}

struct PlaceStructureExecutor;

#[allow(clippy::too_many_lines)]
impl CommandExecutor for PlaceStructureExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let structure_id = context.get_argument::<Identifier>("structure")?;
        let structure_name = structure_id.to_string();

        let key = StructureKeys::from_name(&structure_name).ok_or_else(|| {
            STRUCTURE_INVALID.create_without_context(TextComponent::text(structure_name.clone()))
        })?;

        let structure = Structure::get(&key);

        let block_pos = BlockPosArgumentType::get_block_pos(context, "pos").unwrap_or_else(|_| {
            let p = context.source.position;
            BlockPos::new(p.x as i32, p.y as i32, p.z as i32)
        });

        let seed = hash_block_pos(block_pos.0.x, block_pos.0.y, block_pos.0.z) as u64;

        let mut entities = Vec::new();
        let (_piece_count, placer) = {
            let world_gen = context.world().level.world_gen();
            let settings = NoiseSettings::from_dimension(world_gen.dimension());

            if structure.structure_type == StructureType::Jigsaw {
                let pool = structure.start_pool.ok_or_else(|| {
                    STRUCTURE_INVALID
                        .create_without_context(TextComponent::text(structure_name.clone()))
                })?;
                let size = structure.size.ok_or_else(|| {
                    STRUCTURE_INVALID
                        .create_without_context(TextComponent::text(structure_name.clone()))
                })?;

                let mut random = RandomGenerator::Legacy(LegacyRand::from_seed(seed));
                let pool_alias_lookup =
                    PoolAliasLookup::from_bindings(structure.pool_aliases, &mut random);

                let position = JigsawPlacement::add_pieces(
                    &mut StructureGeneratorContext {
                        seed: seed as i64,
                        chunk_x: block_pos.0.x >> 4,
                        chunk_z: block_pos.0.z >> 4,
                        random,
                        sea_level: settings.sea_level,
                        min_y: (settings.shape.min_y as i32).max(world_gen.dimension().min_y),
                        height: settings
                            .shape
                            .height
                            .min(world_gen.dimension().height as u16),
                        height_sampler: None,
                        structure_key: Some(key),
                    },
                    pool,
                    structure.start_jigsaw_name,
                    size,
                    block_pos,
                    structure.use_expansion_hack.unwrap_or(false),
                    structure.project_start_to_heightmap.is_some(),
                    &MaxDistance::new(structure.max_distance_from_center.unwrap_or(128)),
                    DimensionPadding::ZERO,
                    LiquidSettings::ApplyWaterlog,
                    &pool_alias_lookup,
                )
                .ok_or_else(|| STRUCTURE_FAILED.create_without_context())?;

                let collector = position
                    .collector
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let piece_count = collector.pieces.len();

                let mut placer = WorldBlockPlacer::new(context.world());
                let reference_pos = collector.pieces.first().map_or(block_pos.0, |piece| {
                    start_reference_pos(&piece.bounding_box())
                });
                for piece in &collector.pieces {
                    if let Some(jigsaw_piece) =
                        piece.as_any().downcast_ref::<PoolElementStructurePiece>()
                    {
                        place_pool_element_templates(
                            jigsaw_piece,
                            &mut placer,
                            None,
                            false,
                            reference_pos,
                            context.world().level.seed.0 as i64,
                            Some(&mut |entity| entities.push(entity)),
                        );
                    }
                }

                (piece_count, placer)
            } else {
                let random = RandomGenerator::Legacy(LegacyRand::from_seed(seed));

                let position = pumpkin_world::generation::structure::generate_structure_position(
                    &key,
                    structure,
                    StructureGeneratorContext {
                        seed: seed as i64,
                        chunk_x: block_pos.0.x >> 4,
                        chunk_z: block_pos.0.z >> 4,
                        random,
                        sea_level: settings.sea_level,
                        min_y: (settings.shape.min_y as i32).max(world_gen.dimension().min_y),
                        height: settings
                            .shape
                            .height
                            .min(world_gen.dimension().height as u16),
                        height_sampler: None,
                        structure_key: Some(key),
                    },
                    &mut |_| true,
                )
                .ok_or_else(|| STRUCTURE_FAILED.create_without_context())?;

                let mut collector = position
                    .collector
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let piece_count = collector.pieces.len();

                let mut placer = WorldBlockPlacer::new(context.world());
                let reference_pos = collector.pieces.first().map_or(block_pos.0, |piece| {
                    start_reference_pos(&piece.bounding_box())
                });

                for piece in &collector.pieces {
                    if let Some(jigsaw_piece) =
                        piece.as_any().downcast_ref::<PoolElementStructurePiece>()
                    {
                        place_pool_element_templates(
                            jigsaw_piece,
                            &mut placer,
                            None,
                            false,
                            reference_pos,
                            context.world().level.seed.0 as i64,
                            Some(&mut |entity| entities.push(entity)),
                        );
                    }
                }

                let has_non_jigsaw = collector.pieces.iter().any(|p| {
                    p.as_any()
                        .downcast_ref::<PoolElementStructurePiece>()
                        .is_none()
                });

                if has_non_jigsaw {
                    let reg = CommandBlockRegistry;

                    let mut min_x = i32::MAX;
                    let mut min_z = i32::MAX;
                    let mut max_x = i32::MIN;
                    let mut max_z = i32::MIN;

                    for piece in &collector.pieces {
                        if piece
                            .as_any()
                            .downcast_ref::<PoolElementStructurePiece>()
                            .is_none()
                        {
                            let bb = piece.bounding_box();
                            min_x = min_x.min(bb.min.x);
                            min_z = min_z.min(bb.min.z);
                            max_x = max_x.max(bb.max.x);
                            max_z = max_z.max(bb.max.z);
                        }
                    }

                    let start_cx = min_x >> 4;
                    let end_cx = max_x >> 4;
                    let start_cz = min_z >> 4;
                    let end_cz = max_z >> 4;

                    for cx in start_cx..=end_cx {
                        for cz in start_cz..=end_cz {
                            let mut chunk = ProtoChunk::new(cx, cz, &world_gen);
                            let chunk_min_y = chunk.bottom_y() as i32;
                            let chunk_height = chunk.height() as i32;
                            let surface_y = ground_y(block_pos.0.y, chunk_min_y, chunk_height);

                            // Seed heightmaps and fill below-surface with stone so
                            // pieces that carve through solid terrain have material
                            let ground = surface_y as i16;
                            chunk.flat_surface_height_map = [ground; 256];
                            chunk.flat_ocean_floor_height_map = [ground; 256];
                            chunk.flat_surface_wg_height_map = [ground; 256];
                            chunk.flat_ocean_floor_wg_height_map = [ground; 256];
                            chunk.flat_motion_blocking_height_map = [ground; 256];
                            chunk.flat_motion_blocking_no_leaves_height_map = [ground; 256];

                            for x in 0..CHUNK_DIM {
                                for z in 0..CHUNK_DIM {
                                    for y in chunk_min_y..surface_y {
                                        chunk.set_block_state(x, y, z, Block::STONE.default_state);
                                    }
                                    chunk.set_block_state(
                                        x,
                                        surface_y,
                                        z,
                                        Block::GRASS_BLOCK.default_state,
                                    );
                                }
                            }

                            let chunk_box = BlockBox::new(
                                cx << 4,
                                chunk_min_y,
                                cz << 4,
                                (cx << 4) + CHUNK_DIM - 1,
                                chunk_min_y + chunk_height - 1,
                                (cz << 4) + CHUNK_DIM - 1,
                            );

                            let snapshot =
                                snapshot_blocks(&chunk, cx, cz, chunk_min_y, chunk_height);

                            let mut rng = RandomGenerator::Legacy(LegacyRand::from_seed(
                                chunk_population_seed(cx, cz, seed),
                            ));

                            for piece in &mut collector.pieces {
                                if piece.bounding_box().intersects(&chunk_box) {
                                    piece.place_in_region(
                                        &mut chunk,
                                        &reg,
                                        &mut rng,
                                        seed as i64,
                                        &chunk_box,
                                        reference_pos,
                                    );
                                }
                            }

                            apply_delta(
                                &chunk,
                                &snapshot,
                                cx,
                                cz,
                                chunk_min_y,
                                chunk_height,
                                &mut placer,
                            );

                            for nbt in chunk.pending_block_entities.drain(..) {
                                placer.block_entity_nbts.push(nbt);
                            }
                            entities.append(&mut chunk.take_pending_structure_entities());
                        }
                    }
                }

                (piece_count, placer)
            }
        };

        placer.finalize();
        context
            .world()
            .queue_block_updates(&placer.changed_positions);
        // Vanilla `StructureStart.placeInChunk` places into the level, so the entities join it.
        context.world().spawn_structure_entities(entities);

        context.source.send_feedback(
            pumpkin_macros::translate_cross!(
                translation::java::COMMANDS_PLACE_STRUCTURE_SUCCESS,
                translation::bedrock::COMMANDS_PLACE_SUCCESS,
                TextComponent::text(structure_name),
                TextComponent::text(block_pos.0.x.to_string()),
                TextComponent::text(block_pos.0.y.to_string()),
                TextComponent::text(block_pos.0.z.to_string())
            ),
            true,
        );

        Ok(1)
    }
}

struct PlaceFeatureExecutor;

impl CommandExecutor for PlaceFeatureExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let feature_id = context.get_argument::<Identifier>("feature")?;
        let feature_name = feature_id.to_string();

        let key = PlacedFeatureKey::from_name(&feature_name).ok_or_else(|| {
            FEATURE_INVALID.create_without_context_args_slice(&[
                TextComponent::text(feature_name.clone()),
                TextComponent::text(FEATURE_REGISTRY),
            ])
        })?;

        let placed = PLACED_FEATURES.get(&key).ok_or_else(|| {
            FEATURE_INVALID.create_without_context_args_slice(&[
                TextComponent::text(feature_name.clone()),
                TextComponent::text(FEATURE_REGISTRY),
            ])
        })?;

        let configured = match &placed.feature {
            Feature::Named(name) => CONFIGURED_FEATURES.get(name).ok_or_else(|| {
                FEATURE_INVALID.create_without_context_args_slice(&[
                    TextComponent::text(feature_name.clone()),
                    TextComponent::text(FEATURE_REGISTRY),
                ])
            })?,
            Feature::Inlined(f) => f.as_ref(),
        };

        let block_pos = BlockPosArgumentType::get_block_pos(context, "pos").unwrap_or_else(|_| {
            let p = context.source.position;
            BlockPos::new(p.x as i32, p.y as i32, p.z as i32)
        });

        let world_gen = context.world().level.world_gen();
        let cx = block_pos.0.x >> 4;
        let cz = block_pos.0.z >> 4;
        let mut chunk = ProtoChunk::new(cx, cz, &world_gen);
        let generation_bottom_y = chunk.generation_bottom_y();
        let generation_height = chunk.generation_height();
        let chunk_min_y = chunk.bottom_y() as i32;
        let chunk_height = chunk.height() as i32;
        let surface_y = ground_y(block_pos.0.y, chunk_min_y, chunk_height);

        let ground = surface_y as i16;
        chunk.flat_surface_height_map = [ground; 256];
        chunk.flat_ocean_floor_height_map = [ground; 256];
        chunk.flat_surface_wg_height_map = [ground; 256];
        chunk.flat_ocean_floor_wg_height_map = [ground; 256];
        chunk.flat_motion_blocking_height_map = [ground; 256];
        chunk.flat_motion_blocking_no_leaves_height_map = [ground; 256];

        // Feature generation runs against a synthetic solid terrain, not the
        // live chunk. Features that depend on existing air, caves, or fluids
        // may therefore differ from normal world generation.
        for x in 0..CHUNK_DIM {
            for z in 0..CHUNK_DIM {
                for y in chunk_min_y..surface_y {
                    chunk.set_block_state(x, y, z, Block::STONE.default_state);
                }
                chunk.set_block_state(x, surface_y, z, Block::GRASS_BLOCK.default_state);
            }
        }

        let snapshot = snapshot_blocks(&chunk, cx, cz, chunk_min_y, chunk_height);

        let reg = CommandBlockRegistry;
        let seed = hash_block_pos(block_pos.0.x, block_pos.0.y, block_pos.0.z) as u64;
        let mut random = RandomGenerator::Legacy(LegacyRand::from_seed(seed));

        configured.generate(
            &mut chunk,
            &reg,
            generation_bottom_y,
            generation_height,
            key,
            &mut random,
            block_pos,
        );

        let mut placer = WorldBlockPlacer::new(context.world());
        apply_delta(
            &chunk,
            &snapshot,
            cx,
            cz,
            chunk_min_y,
            chunk_height,
            &mut placer,
        );

        for nbt in chunk.pending_block_entities.drain(..) {
            placer.block_entity_nbts.push(nbt);
        }

        placer.finalize();
        context
            .world()
            .queue_block_updates(&placer.changed_positions);

        context.source.send_feedback(
            pumpkin_macros::translate_cross!(
                translation::java::COMMANDS_PLACE_FEATURE_SUCCESS,
                translation::bedrock::COMMANDS_PLACE_SUCCESS,
                TextComponent::text(feature_name),
                TextComponent::text(block_pos.0.x.to_string()),
                TextComponent::text(block_pos.0.y.to_string()),
                TextComponent::text(block_pos.0.z.to_string())
            ),
            true,
        );

        Ok(1)
    }
}

fn snapshot_blocks(
    chunk: &ProtoChunk,
    cx: i32,
    cz: i32,
    min_y: i32,
    height: i32,
) -> Vec<BlockStateId> {
    let mut snap = Vec::with_capacity((CHUNK_DIM * height * CHUNK_DIM) as usize);
    for x in 0..CHUNK_DIM {
        for y in min_y..min_y + height {
            for z in 0..CHUNK_DIM {
                snap.push(chunk.get_block_state(&Vector3::new((cx << 4) + x, y, (cz << 4) + z)));
            }
        }
    }
    snap
}

fn apply_delta(
    chunk: &ProtoChunk,
    snapshot: &[BlockStateId],
    cx: i32,
    cz: i32,
    min_y: i32,
    height: i32,
    placer: &mut WorldBlockPlacer<'_>,
) {
    // The synthetic chunk is seeded with terrain so structure pieces can carve
    // against it. Applying the delta therefore intentionally propagates air
    // changes too, which can replace real terrain where a piece carved that
    // synthetic stone.
    let mut idx = 0usize;
    for x in 0..CHUNK_DIM {
        for y in min_y..min_y + height {
            for z in 0..CHUNK_DIM {
                let pos = Vector3::new((cx << 4) + x, y, (cz << 4) + z);
                let new_id = chunk.get_block_state(&pos);
                if new_id != snapshot[idx] {
                    placer.set_block_state(&pos, new_id.to_state());
                }
                idx += 1;
            }
        }
    }
}

/// The `/place template` subcommand.
fn template_command() -> LiteralArgumentBuilder {
    literal("template").then(
        argument("template", TemplateNameArgumentType)
            .executes(PlaceTemplateExecutor {
                args: 0,
                strict: false,
            })
            .then(
                argument("pos", BlockPosArgumentType)
                    .executes(PlaceTemplateExecutor {
                        args: 1,
                        strict: false,
                    })
                    .then(
                        argument("rotation", TemplateRotationArgumentType)
                            .executes(PlaceTemplateExecutor {
                                args: 2,
                                strict: false,
                            })
                            .then(
                                argument("mirror", TemplateMirrorArgumentType)
                                    .executes(PlaceTemplateExecutor {
                                        args: 3,
                                        strict: false,
                                    })
                                    .then(
                                        argument("integrity", FloatArgumentType::new(0.0, 1.0))
                                            .executes(PlaceTemplateExecutor {
                                                args: 4,
                                                strict: false,
                                            })
                                            .then(
                                                argument("seed", IntegerArgumentType::any())
                                                    .executes(PlaceTemplateExecutor {
                                                        args: 5,
                                                        strict: false,
                                                    })
                                                    .then(literal("strict").executes(
                                                        PlaceTemplateExecutor {
                                                            args: 5,
                                                            strict: true,
                                                        },
                                                    )),
                                            ),
                                    ),
                            ),
                    ),
            ),
    )
}

pub fn register(dispatcher: &mut CommandDispatcher, registry: &PermissionRegistry) {
    registry.register_permission_or_panic(Permission::new(
        PERMISSION,
        DESCRIPTION,
        PermissionDefault::Op(PermissionLvl::Two),
    ));

    dispatcher.register(
        command("place", DESCRIPTION)
            .requires(PERMISSION)
            .then(template_command())
            .then(
                literal("jigsaw").then(
                    argument(
                        "pool",
                        RegistryIdArgumentType::with_names(
                            JavaClientArgumentType::ResourceKey {
                                identifier: Identifier::vanilla_static("worldgen/template_pool"),
                            },
                            || {
                                pumpkin_world::generation::structure::template::all_pool_names()
                                    .iter()
                                    .map(|name| format!("minecraft:{name}"))
                                    .collect()
                            },
                            &["minecraft:village/plains/houses"],
                        ),
                    )
                    .then(
                        argument("target", IdentifierArgumentType).then(
                            argument("max_depth", IntegerArgumentType::new(1, 20))
                                .executes(PlaceJigsawExecutor)
                                .then(
                                    argument("pos", BlockPosArgumentType)
                                        .executes(PlaceJigsawExecutor),
                                ),
                        ),
                    ),
                ),
            )
            .then(
                literal("structure").then(
                    argument("structure", RegistryIdArgumentType::structure())
                        .executes(PlaceStructureExecutor)
                        .then(
                            argument("pos", BlockPosArgumentType).executes(PlaceStructureExecutor),
                        ),
                ),
            )
            .then(
                literal("feature").then(
                    argument("feature", RegistryIdArgumentType::placed_feature())
                        .executes(PlaceFeatureExecutor)
                        .then(argument("pos", BlockPosArgumentType).executes(PlaceFeatureExecutor)),
                ),
            ),
    );
}
