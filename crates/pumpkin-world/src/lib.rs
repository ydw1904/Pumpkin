#![allow(clippy::unreachable)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use pumpkin_data::{Block, dimension::Dimension};

/// Embeds a JSON fixture
#[cfg(test)]
macro_rules! include_json {
    ($path:literal) => {
        serde_json::from_str(include_str!($path))
            .expect(concat!("Failed to decode JSON fixture: ", $path))
    };
}

pub mod biome;
pub mod block;
pub mod chunk;
pub mod chunk_map;
pub mod chunk_system;
pub mod clip;
pub mod cylindrical_chunk_iterator;
pub mod data;
pub mod dimension;
pub mod generation;
pub mod level;
pub mod lighting;
pub mod poi;
pub mod test_instance;
pub mod tick;
pub mod world;
pub mod world_info;

pub use clip::{BlockHitResult, ClipBlock, ClipContext, ClipFluid};

pub const CURRENT_MC_VERSION: &str = "26.3";
pub const CURRENT_MC_DATA_VERSION: i32 = 5023;
pub const CURRENT_BEDROCK_MC_VERSION: &str = "1.26.63";
pub const CURRENT_BEDROCK_MC_PROTOCOL: u32 = 2225;

/// Most rayon jobs a light parallel step of the tick splits into.
///
/// Each job can wake a sleeping pool thread, and a woken thread searches every other thread's
/// queue many times before it sleeps again. On a large pool that costs far more than splitting a
/// cheap step further saves.
pub const MAX_TICK_STEP_JOBS: usize = 16;

/// The fewest items a rayon job takes in a parallel tick step over `len` items, when a job is
/// worth at least `min` items.
#[must_use]
pub const fn tick_step_min_len(len: usize, min: usize) -> usize {
    let bounded = len.div_ceil(MAX_TICK_STEP_JOBS);
    if bounded > min { bounded } else { min }
}

/// Fewest chunks a per-tick chunk step needs before it runs in parallel.
///
/// A split step makes the tick thread sleep at the join until the stolen half is done, and after
/// each sleep the OS may wake it on another core. Random tick sampling, the costliest of these
/// steps at about 3 µs a chunk, only got faster split from about this many chunks on.
pub const MIN_PARALLEL_TICK_CHUNKS: usize = 64;

/// [`tick_step_min_len`] for a tick step over `len` chunks: below [`MIN_PARALLEL_TICK_CHUNKS`]
/// the step is not split and runs on the calling thread, in the same order.
#[must_use]
pub const fn chunk_step_min_len(len: usize, min: usize) -> usize {
    if len < MIN_PARALLEL_TICK_CHUNKS {
        usize::MAX
    } else {
        tick_step_min_len(len, min)
    }
}

#[macro_export]
macro_rules! global_path {
    ($path:expr) => {{
        use std::path::Path;
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join(file!())
            .parent()
            .unwrap()
            .join($path)
    }};
}

// TODO: is there a way to do in-file benches?
pub use generation::{
    GlobalRandomConfig, noise::router::proto_noise_router::ProtoNoiseRouters,
    proto_chunk::ProtoChunk,
};

use crate::generation::{
    noise::{
        CHUNK_DIM, ChunkNoiseGenerator, aquifer_sampler::FluidLevel,
        router::density_volume::DensityVolume,
    },
    positions::chunk_pos,
};

pub fn bench_create_and_populate_noise(random_config: &GlobalRandomConfig) {
    use crate::generation::generator::{GeneratorInit, VanillaGenerator, WorldGenerator};
    use crate::generation::noise::router::surface_height_sampler::{
        SurfaceHeightEstimateSampler, SurfaceHeightSamplerBuilderOptions,
    };
    use crate::generation::proto_chunk::StandardChunkFluidLevelSampler;
    use pumpkin_util::world_seed::Seed;

    let world_gen = WorldGenerator::Noise(Box::new(VanillaGenerator::new(
        Seed(random_config.seed),
        Dimension::OVERWORLD,
    )));
    let WorldGenerator::Noise(generator) = &world_gen else {
        unreachable!()
    };
    let mut chunk = ProtoChunk::new(0, 0, &world_gen);

    // Create noise sampler and other required components
    let settings = generator.settings;
    let generation_shape = &settings.shape;
    let sampler = StandardChunkFluidLevelSampler::new(
        FluidLevel::new(
            settings.sea_level,
            Block::from_state_id(settings.default_fluid.id),
        ),
        FluidLevel::new(-54, &pumpkin_data::Block::LAVA),
    );

    let start_x = chunk_pos::start_block_x(0);
    let start_z = chunk_pos::start_block_z(0);

    let mut noise_sampler = ChunkNoiseGenerator::new(
        &generator.base_router.noise,
        &generator.random_config,
        DensityVolume::with_block_step(
            CHUNK_DIM as usize,
            generation_shape.height as usize,
            CHUNK_DIM as usize,
            start_x,
            i32::from(generation_shape.min_y),
            start_z,
        ),
        generation_shape,
        sampler,
        settings.aquifers_enabled,
        settings.ore_veins_enabled,
        Vec::new(),
        Vec::new(),
        None,
    );

    // Surface height estimator
    let surface_config = SurfaceHeightSamplerBuilderOptions::new(
        generation_shape.min_y as i32,
        generation_shape.max_y() as i32,
        generation_shape.vertical_cell_block_count() as usize,
    );
    let mut surface_height_estimate_sampler = SurfaceHeightEstimateSampler::generate(
        &generator.base_router.surface_estimator,
        &surface_config,
    );

    chunk.populate_noise(
        generator,
        &mut noise_sampler,
        &generator.random_config.ore_random_deriver,
        &mut surface_height_estimate_sampler,
    );
}

pub fn bench_create_and_populate_biome(random_config: &GlobalRandomConfig) {
    use crate::generation::generator::{GeneratorInit, VanillaGenerator, WorldGenerator};
    use crate::generation::noise::router::multi_noise_sampler::MultiNoiseSampler;
    use pumpkin_util::world_seed::Seed;

    let world_gen = WorldGenerator::Noise(Box::new(VanillaGenerator::new(
        Seed(random_config.seed),
        Dimension::OVERWORLD,
    )));
    let WorldGenerator::Noise(generator) = &world_gen else {
        unreachable!()
    };
    let mut chunk = ProtoChunk::new(0, 0, &world_gen);

    // Create multi-noise sampler
    let mut multi_noise_sampler = MultiNoiseSampler::generate(&generator.base_router.multi_noise);

    chunk.populate_biomes(generator, &mut multi_noise_sampler);
}

pub fn bench_create_and_populate_noise_with_surface(random_config: &GlobalRandomConfig) {
    use crate::chunk_system::{Chunk, generation_cache::SurfaceBiomeNeighborhood};
    use crate::generation::generator::{GeneratorInit, VanillaGenerator, WorldGenerator};
    use crate::generation::noise::router::{
        multi_noise_sampler::MultiNoiseSampler,
        surface_height_sampler::{
            SurfaceHeightEstimateSampler, SurfaceHeightSamplerBuilderOptions,
        },
    };
    use crate::generation::proto_chunk::StandardChunkFluidLevelSampler;
    use pumpkin_util::world_seed::Seed;

    let world_gen = WorldGenerator::Noise(Box::new(VanillaGenerator::new(
        Seed(random_config.seed),
        Dimension::OVERWORLD,
    )));
    let WorldGenerator::Noise(generator) = &world_gen else {
        unreachable!()
    };
    let mut chunk = ProtoChunk::new(0, 0, &world_gen);

    // Create all required components
    let settings = generator.settings;
    let generation_shape = &settings.shape;
    let start_x = chunk_pos::start_block_x(0);
    let start_z = chunk_pos::start_block_z(0);

    // Multi-noise sampler for biomes
    let mut multi_noise_sampler = MultiNoiseSampler::generate(&generator.base_router.multi_noise);

    // Noise sampler
    let sampler = StandardChunkFluidLevelSampler::new(
        FluidLevel::new(
            settings.sea_level,
            Block::from_state_id(settings.default_fluid.id),
        ),
        FluidLevel::new(-54, &Block::LAVA),
    );

    let mut noise_sampler = ChunkNoiseGenerator::new(
        &generator.base_router.noise,
        &generator.random_config,
        DensityVolume::with_block_step(
            CHUNK_DIM as usize,
            generation_shape.height as usize,
            CHUNK_DIM as usize,
            start_x,
            i32::from(generation_shape.min_y),
            start_z,
        ),
        generation_shape,
        sampler,
        settings.aquifers_enabled,
        settings.ore_veins_enabled,
        Vec::new(),
        Vec::new(),
        None,
    );

    // Surface height estimator
    let surface_config = SurfaceHeightSamplerBuilderOptions::new(
        generation_shape.min_y as i32,
        generation_shape.max_y() as i32,
        generation_shape.vertical_cell_block_count() as usize,
    );
    let mut surface_height_estimate_sampler = SurfaceHeightEstimateSampler::generate(
        &generator.base_router.surface_estimator,
        &surface_config,
    );

    chunk.populate_biomes(generator, &mut multi_noise_sampler);

    // Surface biome zoom may select a quart from an immediate neighbor. Build the same read-only
    // palette snapshot that the runtime scheduler supplies to a Surface task.
    let mut surface_biomes = SurfaceBiomeNeighborhood::new(0, 0);
    for chunk_x in -1..=1 {
        for chunk_z in -1..=1 {
            if chunk_x == 0 && chunk_z == 0 {
                continue;
            }
            let mut neighbor = ProtoChunk::new(chunk_x, chunk_z, &world_gen);
            neighbor.step_to_biomes(generator);
            assert!(surface_biomes.push_chunk(&Chunk::Proto(Box::new(neighbor))));
        }
    }
    let center = Chunk::Proto(Box::new(chunk));
    assert!(surface_biomes.push_chunk(&center));
    let Chunk::Proto(mut chunk) = center else {
        unreachable!()
    };

    chunk.populate_noise(
        generator,
        &mut noise_sampler,
        &generator.random_config.ore_random_deriver,
        &mut surface_height_estimate_sampler,
    );
    chunk.build_surface(
        generator,
        &surface_biomes,
        &mut surface_height_estimate_sampler,
    );
}
