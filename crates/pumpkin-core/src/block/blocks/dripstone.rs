use std::sync::Arc;

use crate::{
    block::{
        BlockBehaviour, CanPlaceAtArgs, GetStateForNeighborUpdateArgs, OnLandedUponArgs,
        OnPlaceArgs, OnProjectileHitArgs, OnScheduledTickArgs, PathComputationType, RandomTickArgs,
        blocks::cauldron::can_receive_stalactite_drip, push_entities_up,
    },
    entity::{EntityBase, falling::FallingEntity, player::Player},
    world::{World, game_event::GameEventContext},
};
use pumpkin_data::{
    Block, BlockDirection, BlockState, BlockStateId,
    block_properties::{PointedDripstoneLikeProperties, SpeleothemThickness, VerticalDirection},
    damage::DamageType,
    entity::EntityType,
    fluid::Fluid,
    game_event::GameEvent,
    tag::{self, Taggable},
    world::WorldEvent,
};
use pumpkin_macros::pumpkin_block_from_tag;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::random::gameplay::game_random;
use pumpkin_world::{
    tick::TickPriority,
    world::{BlockAccessor, BlockFlags},
};

#[pumpkin_block_from_tag("minecraft:speleothems")]
pub struct DripstoneBlock;

impl BlockBehaviour for DripstoneBlock {
    fn can_place_at(&self, args: CanPlaceAtArgs<'_>) -> bool {
        // Without a placement, vanilla `canSurvive` checks the direction the state already has.
        if args.direction.is_none() && args.player.is_none() {
            return can_survive(
                args.block_accessor,
                args.block,
                args.position,
                args.state.id,
            );
        }
        can_place_at_pos(
            args.block_accessor,
            args.block,
            args.position,
            args.direction,
            args.player,
        )
    }

    fn on_place(&self, args: OnPlaceArgs<'_>) -> BlockStateId {
        let mut dripstone_props = PointedDripstoneLikeProperties::default(args.block);
        dripstone_props.waterlogged = args.replacing.water_source();
        let Some(support_block_ver_dir) = get_support_block_vertical_direction(
            args.world,
            args.block,
            args.position,
            Some(args.direction),
            Some(args.player),
        ) else {
            return Block::AIR.default_state.id;
        };

        dripstone_props.vertical_direction = flip_dir(support_block_ver_dir);
        // `getStateForPlacement`: sneaking keeps an opposing tip unmerged.
        dripstone_props.thickness = calculate_speleothem_thickness(
            args.world,
            args.block,
            args.position,
            dripstone_props.vertical_direction,
            !args.player.get_entity().is_sneaking(),
        );
        dripstone_props.to_state_id(args.block)
    }

    /// Vanilla `SpeleothemBlock.updateShape`.
    fn get_state_for_neighbor_update(
        &self,
        args: GetStateForNeighborUpdateArgs<'_>,
    ) -> BlockStateId {
        let mut dripstone_props = PointedDripstoneLikeProperties::from_state_id(args.state_id);
        if dripstone_props.waterlogged {
            args.world.schedule_fluid_tick(
                &Fluid::WATER,
                *args.position,
                Fluid::WATER.flow_speed,
                TickPriority::Normal,
            );
        }
        let Some(neighbor_dir) = block_direction_to_vertical_direction(args.direction) else {
            return args.state_id;
        };
        let tip_dir = dripstone_props.vertical_direction;
        if tip_dir == VerticalDirection::Down
            && args
                .world
                .is_block_tick_scheduled(args.position, args.block)
        {
            return args.state_id;
        }
        if neighbor_dir == flip_dir(tip_dir)
            && !can_survive(args.world, args.block, args.position, args.state_id)
        {
            let delay = if tip_dir == VerticalDirection::Down {
                DELAY_BEFORE_FALLING
            } else {
                1
            };
            args.world
                .schedule_block_tick(args.block, *args.position, delay, TickPriority::Normal);
            return args.state_id;
        }
        let merge_opposing_tips = dripstone_props.thickness == SpeleothemThickness::TipMerge;
        dripstone_props.thickness = calculate_speleothem_thickness(
            args.world,
            args.block,
            args.position,
            tip_dir,
            merge_opposing_tips,
        );
        dripstone_props.to_state_id(args.block)
    }

    /// Vanilla `SpeleothemBlock.tick`.
    fn on_scheduled_tick(&self, args: OnScheduledTickArgs<'_>) {
        let state_id = args.world.get_block_state_id(args.position);
        if is_speleothem_with_direction(state_id, VerticalDirection::Up)
            && !can_survive(args.world.as_ref(), args.block, args.position, state_id)
        {
            args.world
                .destroy_block(args.position, None, BlockFlags::NOTIFY_ALL);
        } else {
            spawn_falling_stalactite(args.world, args.position, state_id);
        }
    }

    /// Vanilla `PointedDripstoneBlock.fallOn`.
    fn on_landed_upon(&self, args: OnLandedUponArgs<'_>) {
        let Some(living) = args.entity.get_living_entity() else {
            return;
        };
        let pos = args.entity.get_entity().get_on_pos_legacy();
        let (block, state_id) = args.world.get_block_and_state_id(&pos);
        let stalagmite_tip = block == &Block::POINTED_DRIPSTONE && {
            let props = PointedDripstoneLikeProperties::from_state_id(state_id);
            props.vertical_direction == VerticalDirection::Up
                && props.thickness == SpeleothemThickness::Tip
        };
        if stalagmite_tip {
            living.handle_fall_damage(
                args.entity,
                args.fall_distance + 2.5,
                2.0,
                DamageType::STALAGMITE,
            );
        } else {
            living.handle_fall_damage(args.entity, args.fall_distance, 1.0, DamageType::FALL);
        }
    }

    /// Vanilla `PointedDripstoneBlock.onProjectileHit`: a fast trident breaks it.
    fn on_projectile_hit(&self, args: OnProjectileHitArgs<'_>) {
        const TRIDENT_BREAK_MIN_SPEED: f64 = 0.6;

        let projectile = args.projectile.get_entity();
        if args.block != &Block::POINTED_DRIPSTONE
            || projectile.entity_type != &EntityType::TRIDENT
            || projectile.velocity.load().length() <= TRIDENT_BREAK_MIN_SPEED
        {
            return;
        }
        let game_rules = &args.world.level_info.load().game_rules;
        // `Projectile.mayInteract`: a player owner must be allowed to build here, an owning mob
        // needs mob griefing. `mayBreak`: tridents are impact projectiles.
        let owner = args
            .projectile
            .get_projectile_owner()
            .and_then(|owner| owner.owner().get(args.world));
        let may_interact = owner.as_ref().is_none_or(|owner| {
            owner
                .get_player()
                .map_or(game_rules.mob_griefing, |player| {
                    !args.world.is_in_spawn_protection(player, args.position)
                })
        });
        if may_interact && game_rules.projectiles_can_break_blocks {
            args.world
                .destroy_block(args.position, None, BlockFlags::NOTIFY_ALL);
        }
    }

    fn is_pathfindable(&self, _state: &BlockState, _computation_type: PathComputationType) -> bool {
        false
    }

    /// Vanilla `PointedDripstoneBlock.randomTick`. Stalactite growth isn't ported.
    fn random_tick(&self, args: RandomTickArgs<'_>) {
        let state_id = args.world.get_block_state_id(args.position);
        if args.block == &Block::POINTED_DRIPSTONE {
            maybe_transfer_fluid(
                args.world,
                args.position,
                state_id,
                game_random().next_f32(),
            );
        }
        // Vanilla `SpeleothemBlock.randomTick`.
        if game_random().next_f32() < GROWTH_PROBABILITY_PER_RANDOM_TICK
            && is_stalactite_start_pos(args.world, args.position, state_id)
        {
            grow_stalactite_or_stalagmite_if_possible(
                args.world,
                args.block,
                args.position,
                state_id,
            );
        }
    }
}

/// Vanilla `PointedDripstoneBlock.WATER_TRANSFER_PROBABILITY_PER_RANDOM_TICK`.
const WATER_TRANSFER_PROBABILITY_PER_RANDOM_TICK: f32 = 0.175_781_25;
/// Vanilla `PointedDripstoneBlock.LAVA_TRANSFER_PROBABILITY_PER_RANDOM_TICK`.
const LAVA_TRANSFER_PROBABILITY_PER_RANDOM_TICK: f32 = 0.058_593_75;
/// Vanilla `PointedDripstoneBlock.MAX_SEARCH_LENGTH_WHEN_CHECKING_DRIP_TYPE`.
const MAX_SEARCH_LENGTH_WHEN_CHECKING_DRIP_TYPE: i32 = 11;
/// Vanilla `PointedDripstoneBlock.MAX_SEARCH_LENGTH_BETWEEN_STALACTITE_TIP_AND_CAULDRON`.
const MAX_SEARCH_LENGTH_BETWEEN_STALACTITE_TIP_AND_CAULDRON: i32 = 11;

/// Vanilla `SpeleothemBlock.DELAY_BEFORE_FALLING`.
const DELAY_BEFORE_FALLING: u32 = 2;
/// Vanilla `SpeleothemBlock.STALACTITE_DAMAGE_PER_FALL_DISTANCE_AND_SIZE`.
const STALACTITE_DAMAGE_PER_FALL_DISTANCE_AND_SIZE: f32 = 1.0;
/// Vanilla `SpeleothemBlock.STALACTITE_MAX_DAMAGE`.
const STALACTITE_MAX_DAMAGE: i32 = 40;
/// Vanilla `SpeleothemBlock.MAX_STALACTITE_HEIGHT_FOR_DAMAGE_CALCULATION`.
const MAX_STALACTITE_HEIGHT_FOR_DAMAGE_CALCULATION: i32 = 6;
/// Vanilla `SpeleothemBlock.GROWTH_PROBABILITY_PER_RANDOM_TICK`.
const GROWTH_PROBABILITY_PER_RANDOM_TICK: f32 = 0.011_377_778;
/// Vanilla `SpeleothemBlock.MAX_STALAGMITE_SEARCH_RANGE_WHEN_GROWING`.
const MAX_STALAGMITE_SEARCH_RANGE_WHEN_GROWING: i32 = 10;

/// Vanilla `SpeleothemBlock.calculateSpeleothemThickness`.
fn calculate_speleothem_thickness(
    block_accessor: &dyn BlockAccessor,
    speleothem: &Block,
    pos: &BlockPos,
    tip_dir: VerticalDirection,
    merge_opposing_tips: bool,
) -> SpeleothemThickness {
    let base_dir = flip_dir(tip_dir);
    let in_front = block_accessor.get_block_state_id(&relative_vertical(pos, tip_dir));
    if is_speleothem_with_direction(in_front, base_dir) && in_front.to_block() == speleothem {
        let in_front_thickness = PointedDripstoneLikeProperties::from_state_id(in_front).thickness;
        return if !merge_opposing_tips && in_front_thickness != SpeleothemThickness::TipMerge {
            SpeleothemThickness::Tip
        } else {
            SpeleothemThickness::TipMerge
        };
    }
    // Unlike the opposing tip above, any speleothem counts from here on.
    if !is_speleothem_with_direction(in_front, tip_dir) {
        return SpeleothemThickness::Tip;
    }
    let in_front_thickness = PointedDripstoneLikeProperties::from_state_id(in_front).thickness;
    if matches!(
        in_front_thickness,
        SpeleothemThickness::Tip | SpeleothemThickness::TipMerge
    ) {
        return SpeleothemThickness::Frustum;
    }
    let behind = block_accessor.get_block_state_id(&relative_vertical(pos, base_dir));
    if is_speleothem_with_direction(behind, tip_dir) {
        SpeleothemThickness::Middle
    } else {
        SpeleothemThickness::Base
    }
}

/// Vanilla `SpeleothemBlock.isSpeleothemWithDirection`.
fn is_speleothem_with_direction(state_id: BlockStateId, tip_dir: VerticalDirection) -> bool {
    state_id
        .to_block()
        .has_tag(&tag::Block::MINECRAFT_SPELEOTHEMS)
        && PointedDripstoneLikeProperties::from_state_id(state_id).vertical_direction == tip_dir
}

/// Vanilla `SpeleothemBlock.spawnFallingStalactite`: the stalactite falls from `pos` down to its
/// tip, and only the tip hurts what it lands on.
fn spawn_falling_stalactite(world: &Arc<World>, pos: &BlockPos, state_id: BlockStateId) {
    let mut fall_pos = *pos;
    let mut fall_state = state_id;
    while is_stalactite(fall_state) {
        let entity = FallingEntity::replace_spawn(world, fall_pos, fall_state);
        if is_tip(fall_state, true) {
            let size =
                (1 + pos.0.y - fall_pos.0.y).max(MAX_STALACTITE_HEIGHT_FOR_DAMAGE_CALCULATION);
            entity.set_hurts_entities(
                STALACTITE_DAMAGE_PER_FALL_DISTANCE_AND_SIZE * size as f32,
                STALACTITE_MAX_DAMAGE,
            );
            break;
        }
        fall_pos = fall_pos.down();
        fall_state = world.get_block_state_id(&fall_pos);
    }
}

/// Vanilla `SpeleothemBlock.growStalactiteOrStalagmiteIfPossible`.
fn grow_stalactite_or_stalagmite_if_possible(
    world: &Arc<World>,
    speleothem: &Block,
    start: &BlockPos,
    start_state: BlockStateId,
) {
    if !can_grow(world, speleothem, start) {
        return;
    }
    // `getMaxGrowthLength`: `SulfurSpikeBlock` overrides it.
    let max_growth_length = if speleothem == &Block::SULFUR_SPIKE {
        2
    } else {
        7
    };
    let Some(tip) = find_tip(world, start, start_state, max_growth_length, false) else {
        return;
    };
    let tip_state = world.get_block_state_id(&tip);
    if !is_free_hanging_stalactite(tip_state) || !can_tip_grow(world, speleothem, &tip, tip_state) {
        return;
    }
    if game_random().random::<bool>() {
        grow(world, speleothem, &tip, VerticalDirection::Down);
    } else {
        grow_stalagmite_below(world, speleothem, &tip);
    }
}

/// Vanilla `SpeleothemBlock.canGrow`, with `PointedDripstoneBlock`'s water source two above.
fn can_grow(world: &World, speleothem: &Block, pos: &BlockPos) -> bool {
    let grows_on = if speleothem == &Block::SULFUR_SPIKE {
        &Block::SULFUR
    } else {
        &Block::DRIPSTONE_BLOCK
    };
    world.get_block(&pos.up()) == grows_on
        && (speleothem != &Block::POINTED_DRIPSTONE || {
            let (fluid, fluid_state) = world.get_fluid_and_fluid_state(&pos.up_height(2));
            fluid.has_tag(&tag::Fluid::MINECRAFT_WATER) && fluid_state.is_source
        })
}

/// Vanilla `SpeleothemBlock.canTipGrow`.
fn can_tip_grow(
    world: &World,
    speleothem: &Block,
    tip: &BlockPos,
    tip_state: BlockStateId,
) -> bool {
    let direction = PointedDripstoneLikeProperties::from_state_id(tip_state).vertical_direction;
    let grow_state = world.get_block_state_id(&relative_vertical(tip, direction));
    if *World::get_fluid_type_from_state_id(grow_state) != Fluid::EMPTY {
        return false;
    }
    grow_state.to_state().is_air()
        || is_unmerged_tip_with_direction(grow_state, speleothem, flip_dir(direction))
}

/// Vanilla `SpeleothemBlock.isUnmergedTipWithDirection`.
fn is_unmerged_tip_with_direction(
    state_id: BlockStateId,
    speleothem: &Block,
    direction: VerticalDirection,
) -> bool {
    is_tip(state_id, false) && same_speleothem(state_id, speleothem, direction)
}

/// Vanilla `SpeleothemBlock.grow`.
fn grow(world: &Arc<World>, speleothem: &Block, from: &BlockPos, direction: VerticalDirection) {
    let target = relative_vertical(from, direction);
    let existing = world.get_block_state_id(&target);
    if is_unmerged_tip_with_direction(existing, speleothem, flip_dir(direction)) {
        // `createMergedTips`.
        let (stalactite, stalagmite) = if direction == VerticalDirection::Down {
            (target.up(), target)
        } else {
            (target, target.down())
        };
        create_speleothem(
            world,
            speleothem,
            &stalactite,
            VerticalDirection::Down,
            SpeleothemThickness::TipMerge,
        );
        create_speleothem(
            world,
            speleothem,
            &stalagmite,
            VerticalDirection::Up,
            SpeleothemThickness::TipMerge,
        );
    } else if existing.to_state().is_air() || existing.to_block() == &Block::WATER {
        create_speleothem(
            world,
            speleothem,
            &target,
            direction,
            SpeleothemThickness::Tip,
        );
    }
}

/// Vanilla `SpeleothemBlock.createSpeleothem`.
fn create_speleothem(
    world: &Arc<World>,
    speleothem: &Block,
    pos: &BlockPos,
    direction: VerticalDirection,
    thickness: SpeleothemThickness,
) {
    let mut props = PointedDripstoneLikeProperties::default(speleothem);
    props.vertical_direction = direction;
    props.thickness = thickness;
    props.waterlogged =
        *World::get_fluid_type_from_state_id(world.get_block_state_id(pos)) == Fluid::WATER;
    world.set_block_state(pos, props.to_state_id(speleothem), BlockFlags::NOTIFY_ALL);
}

/// Vanilla `SpeleothemBlock.growStalagmiteBelow`.
fn grow_stalagmite_below(world: &Arc<World>, speleothem: &Block, above: &BlockPos) {
    let mut pos = *above;
    for _ in 0..MAX_STALAGMITE_SEARCH_RANGE_WHEN_GROWING {
        pos = pos.down();
        let state_id = world.get_block_state_id(&pos);
        if *World::get_fluid_type_from_state_id(state_id) != Fluid::EMPTY {
            return;
        }
        if is_unmerged_tip_with_direction(state_id, speleothem, VerticalDirection::Up)
            && can_tip_grow(world, speleothem, &pos, state_id)
        {
            grow(world, speleothem, &pos, VerticalDirection::Up);
            return;
        }
        // `isValidSpeleothemPlacement(pos, UP)` and not `isWaterAt(pos.below())`.
        if can_support_dripstone(
            world.as_ref(),
            speleothem,
            &pos.down(),
            VerticalDirection::Down,
        ) && !world
            .get_fluid(&pos.down())
            .has_tag(&tag::Fluid::MINECRAFT_WATER)
        {
            grow(world, speleothem, &pos.down(), VerticalDirection::Up);
            return;
        }
        // `blocksStalagmiteScan`: only pointed dripstone stops at what it can't drip through.
        if speleothem == &Block::POINTED_DRIPSTONE && !can_drip_through(state_id) {
            return;
        }
    }
}

/// Vanilla `PointedDripstoneBlock.FluidInfo`: the fluid a stalactite drips and where it comes from.
struct FluidInfo {
    pos: BlockPos,
    fluid: &'static Fluid,
    source_state: BlockStateId,
}

/// Vanilla `PointedDripstoneBlock.maybeTransferFluid`: a stalactite turns mud above it into clay,
/// or drips into the cauldron below its tip.
fn maybe_transfer_fluid(world: &Arc<World>, pos: &BlockPos, state_id: BlockStateId, random: f32) {
    // Vanilla also tests the lava chance here, which is the smaller one.
    if random > WATER_TRANSFER_PROBABILITY_PER_RANDOM_TICK {
        return;
    }
    if !is_stalactite_start_pos(world, pos, state_id) {
        return;
    }
    let Some(info) = get_fluid_above_stalactite(world, pos, state_id) else {
        return;
    };
    let transfer_probability = if *info.fluid == Fluid::WATER {
        WATER_TRANSFER_PROBABILITY_PER_RANDOM_TICK
    } else if *info.fluid == Fluid::LAVA {
        LAVA_TRANSFER_PROBABILITY_PER_RANDOM_TICK
    } else {
        return;
    };
    if random >= transfer_probability {
        return;
    }
    let Some(tip) = find_tip(
        world,
        pos,
        state_id,
        MAX_SEARCH_LENGTH_WHEN_CHECKING_DRIP_TYPE,
        false,
    ) else {
        return;
    };
    if info.source_state.to_block() == &Block::MUD && *info.fluid == Fluid::WATER {
        let clay = Block::CLAY.default_state.id;
        push_entities_up(world, info.source_state.to_state(), clay, &info.pos);
        world.set_block_state(&info.pos, clay, BlockFlags::NOTIFY_ALL);
        world.emit_game_event_with_context(
            GameEvent::BlockChange.name(),
            info.pos.to_centered_f64(),
            &GameEventContext {
                source_entity: None,
                affected_state: Some(clay),
            },
        );
        world.sync_world_event(WorldEvent::DripstoneDrip, tip, 0);
    } else if let Some(cauldron_pos) =
        find_fillable_cauldron_below_stalactite_tip(world, &tip, info.fluid)
    {
        world.sync_world_event(WorldEvent::DripstoneDrip, tip, 0);
        let fall_distance = tip.0.y - cauldron_pos.0.y;
        world.schedule_block_tick(
            world.get_block(&cauldron_pos),
            cauldron_pos,
            (50 + fall_distance) as u32,
            TickPriority::Normal,
        );
    }
}

/// Vanilla `SpeleothemBlock.isStalactite`.
fn is_stalactite(state_id: BlockStateId) -> bool {
    is_speleothem_with_direction(state_id, VerticalDirection::Down)
}

/// Vanilla `SpeleothemBlock.isStalactiteStartPos`: the top of a stalactite.
fn is_stalactite_start_pos(world: &World, pos: &BlockPos, state_id: BlockStateId) -> bool {
    is_stalactite(state_id) && world.get_block(&pos.up()) != state_id.to_block()
}

/// Vanilla `SpeleothemBlock.isFreeHangingStalactite`.
fn is_free_hanging_stalactite(state_id: BlockStateId) -> bool {
    if !is_stalactite(state_id) {
        return false;
    }
    let props = PointedDripstoneLikeProperties::from_state_id(state_id);
    props.thickness == SpeleothemThickness::Tip && !props.waterlogged
}

/// Vanilla `SpeleothemBlock.isTip`.
fn is_tip(state_id: BlockStateId, include_merged_tip: bool) -> bool {
    if !state_id
        .to_block()
        .has_tag(&tag::Block::MINECRAFT_SPELEOTHEMS)
    {
        return false;
    }
    let thickness = PointedDripstoneLikeProperties::from_state_id(state_id).thickness;
    thickness == SpeleothemThickness::Tip
        || include_merged_tip && thickness == SpeleothemThickness::TipMerge
}

/// Vanilla `SpeleothemBlock.findBlockVertical`: the first block matching `target` within
/// `max_steps - 1` blocks up or down, through blocks matching `path`.
fn find_block_vertical(
    world: &World,
    pos: &BlockPos,
    up: bool,
    path: impl Fn(BlockStateId) -> bool,
    target: impl Fn(BlockStateId) -> bool,
    max_steps: i32,
) -> Option<BlockPos> {
    let mut cursor = *pos;
    for _ in 1..max_steps {
        cursor = if up { cursor.up() } else { cursor.down() };
        let state_id = world.get_block_state_id(&cursor);
        if target(state_id) {
            return Some(cursor);
        }
        let y = cursor.0.y;
        let outside_build_height =
            y < world.dimension.min_y || y >= world.dimension.min_y + world.dimension.height;
        if outside_build_height || !path(state_id) {
            return None;
        }
    }
    None
}

/// A speleothem of the same block pointing the same way.
fn same_speleothem(state_id: BlockStateId, block: &Block, direction: VerticalDirection) -> bool {
    state_id.to_block() == block
        && PointedDripstoneLikeProperties::from_state_id(state_id).vertical_direction == direction
}

/// Vanilla `SpeleothemBlock.findTip`.
fn find_tip(
    world: &World,
    pos: &BlockPos,
    state_id: BlockStateId,
    max_search_length: i32,
    include_merged_tip: bool,
) -> Option<BlockPos> {
    if is_tip(state_id, include_merged_tip) {
        return Some(*pos);
    }
    let block = state_id.to_block();
    let direction = PointedDripstoneLikeProperties::from_state_id(state_id).vertical_direction;
    find_block_vertical(
        world,
        pos,
        direction == VerticalDirection::Up,
        |state| same_speleothem(state, block, direction),
        |state| is_tip(state, include_merged_tip),
        max_search_length,
    )
}

/// Vanilla `PointedDripstoneBlock.findRootBlock`: the block the speleothem grows from.
fn find_root_block(
    world: &World,
    pos: &BlockPos,
    state_id: BlockStateId,
    max_search_length: i32,
) -> Option<BlockPos> {
    let block = state_id.to_block();
    let direction = PointedDripstoneLikeProperties::from_state_id(state_id).vertical_direction;
    find_block_vertical(
        world,
        pos,
        direction == VerticalDirection::Down,
        |state| same_speleothem(state, block, direction),
        |state| state.to_block() != block,
        max_search_length,
    )
}

/// Vanilla `PointedDripstoneBlock.getFluidAboveStalactite`: mud counts as water unless water
/// evaporates in the dimension.
fn get_fluid_above_stalactite(
    world: &World,
    pos: &BlockPos,
    state_id: BlockStateId,
) -> Option<FluidInfo> {
    if !is_stalactite(state_id) {
        return None;
    }
    let root = find_root_block(
        world,
        pos,
        state_id,
        MAX_SEARCH_LENGTH_WHEN_CHECKING_DRIP_TYPE,
    )?;
    let above = root.up();
    let source_state = world.get_block_state_id(&above);
    let fluid = if source_state.to_block() == &Block::MUD && !world.dimension.water_evaporates {
        &Fluid::WATER
    } else {
        World::get_fluid_type_from_state_id(source_state)
    };
    Some(FluidInfo {
        pos: above,
        fluid,
        source_state,
    })
}

/// Vanilla `PointedDripstoneBlock.canDripThrough`.
fn can_drip_through(state_id: BlockStateId) -> bool {
    let state = state_id.to_state();
    if state.is_air() {
        return true;
    }
    if state.is_solid_render() || *World::get_fluid_type_from_state_id(state_id) != Fluid::EMPTY {
        return false;
    }
    // `REQUIRED_SPACE_TO_DRIP_THROUGH_NON_SOLID_BLOCK`, `Block.column(4.0, 0.0, 16.0)`.
    !state.get_block_collision_shapes().any(|shape| {
        shape.min.x < 0.625
            && shape.max.x > 0.375
            && shape.min.z < 0.625
            && shape.max.z > 0.375
            && shape.min.y < 1.0
            && shape.max.y > 0.0
    })
}

/// Vanilla `PointedDripstoneBlock.findFillableCauldronBelowStalactiteTip`.
fn find_fillable_cauldron_below_stalactite_tip(
    world: &World,
    tip: &BlockPos,
    fluid: &Fluid,
) -> Option<BlockPos> {
    find_block_vertical(
        world,
        tip,
        false,
        can_drip_through,
        |state| can_receive_stalactite_drip(state.to_block(), fluid),
        MAX_SEARCH_LENGTH_BETWEEN_STALACTITE_TIP_AND_CAULDRON,
    )
}

/// Vanilla `PointedDripstoneBlock.findStalactiteTipAboveCauldron`.
pub fn find_stalactite_tip_above_cauldron(world: &World, cauldron: &BlockPos) -> Option<BlockPos> {
    find_block_vertical(
        world,
        cauldron,
        true,
        can_drip_through,
        is_free_hanging_stalactite,
        MAX_SEARCH_LENGTH_BETWEEN_STALACTITE_TIP_AND_CAULDRON,
    )
}

/// Vanilla `PointedDripstoneBlock.getCauldronFillFluidType`: water or lava, if the stalactite
/// drips either.
pub fn get_cauldron_fill_fluid_type(
    world: &World,
    stalactite: &BlockPos,
) -> Option<&'static Fluid> {
    get_fluid_above_stalactite(world, stalactite, world.get_block_state_id(stalactite))
        .map(|info| info.fluid)
        .filter(|fluid| **fluid == Fluid::WATER || **fluid == Fluid::LAVA)
}

/// Vanilla `SpeleothemBlock.canSurvive`: `isValidSpeleothemPlacement` toward the state's tip.
fn can_survive(
    block_accessor: &dyn BlockAccessor,
    speleothem: &Block,
    pos: &BlockPos,
    state_id: BlockStateId,
) -> bool {
    let support_dir =
        flip_dir(PointedDripstoneLikeProperties::from_state_id(state_id).vertical_direction);
    can_support_dripstone(
        block_accessor,
        speleothem,
        &relative_vertical(pos, support_dir),
        support_dir,
    )
}

fn can_place_at_pos(
    block_accessor: &dyn BlockAccessor,
    speleothem: &Block,
    position: &BlockPos,
    placing_direction: Option<BlockDirection>,
    player_option: Option<&Player>,
) -> bool {
    let Some(support_block_vertical_direction) = get_support_block_vertical_direction(
        block_accessor,
        speleothem,
        position,
        placing_direction,
        player_option,
    ) else {
        return false;
    };
    let support_pos = match support_block_vertical_direction {
        VerticalDirection::Up => position.up(),
        VerticalDirection::Down => position.down(),
    };
    can_support_dripstone(
        block_accessor,
        speleothem,
        &support_pos,
        support_block_vertical_direction,
    )
}

fn get_support_block_vertical_direction(
    block_accessor: &dyn BlockAccessor,
    speleothem: &Block,
    position: &BlockPos,
    placing_direction_wrapper: Option<BlockDirection>,
    player_option: Option<&Player>,
) -> Option<VerticalDirection> {
    let Some(placing_direction) = placing_direction_wrapper else {
        let (block, state) = block_accessor.get_block_and_state(position);
        if block != speleothem {
            return None;
        }
        let props = PointedDripstoneLikeProperties::from_state_id(state.id);
        return Some(flip_dir(props.vertical_direction));
    };
    match block_direction_to_vertical_direction(placing_direction) {
        Some(ver_dir) => match ver_dir {
            VerticalDirection::Up => {
                if can_support_dripstone(
                    block_accessor,
                    speleothem,
                    &position.up(),
                    VerticalDirection::Up,
                ) {
                    return Some(VerticalDirection::Up);
                } else if can_support_dripstone(
                    block_accessor,
                    speleothem,
                    &position.down(),
                    VerticalDirection::Down,
                ) {
                    return Some(VerticalDirection::Down);
                }
                None
            }
            VerticalDirection::Down => {
                if can_support_dripstone(
                    block_accessor,
                    speleothem,
                    &position.down(),
                    VerticalDirection::Down,
                ) {
                    return Some(VerticalDirection::Down);
                } else if can_support_dripstone(
                    block_accessor,
                    speleothem,
                    &position.up(),
                    VerticalDirection::Up,
                ) {
                    return Some(VerticalDirection::Up);
                }
                None
            }
        },
        None => player_option.map_or(Some(VerticalDirection::Up), |player| {
            let (_, pitch) = player.rotation();
            let (can_place_above, can_place_below) = (
                can_support_dripstone(
                    block_accessor,
                    speleothem,
                    &position.up(),
                    VerticalDirection::Up,
                ),
                can_support_dripstone(
                    block_accessor,
                    speleothem,
                    &position.down(),
                    VerticalDirection::Down,
                ),
            );
            match (can_place_above, can_place_below) {
                (true, true) => {
                    if pitch > 0.0 {
                        Some(VerticalDirection::Down)
                    } else {
                        Some(VerticalDirection::Up)
                    }
                }
                (false, false) => None,
                (true, false) => Some(VerticalDirection::Up),
                (false, true) => Some(VerticalDirection::Down),
            }
        }),
    }
}

fn can_support_dripstone(
    block_accessor: &dyn BlockAccessor,
    speleothem: &Block,
    support_pos: &BlockPos,
    support_dir: VerticalDirection,
) -> bool {
    let (support_block, support_state) = block_accessor.get_block_and_state(support_pos);
    if support_block == speleothem {
        let props = PointedDripstoneLikeProperties::from_state_id(support_state.id);
        return props.vertical_direction == flip_dir(support_dir);
    }
    let face = match support_dir {
        VerticalDirection::Up => BlockDirection::Down,
        VerticalDirection::Down => BlockDirection::Up,
    };
    support_state.is_side_solid(face)
}

/// Vanilla `BlockPos.relative` for a vertical direction.
fn relative_vertical(pos: &BlockPos, ver_dir: VerticalDirection) -> BlockPos {
    match ver_dir {
        VerticalDirection::Up => pos.up(),
        VerticalDirection::Down => pos.down(),
    }
}

const fn block_direction_to_vertical_direction(dir: BlockDirection) -> Option<VerticalDirection> {
    match dir {
        BlockDirection::Up => Some(VerticalDirection::Up),
        BlockDirection::Down => Some(VerticalDirection::Down),
        _ => None,
    }
}

fn flip_dir(dir: VerticalDirection) -> VerticalDirection {
    if dir == VerticalDirection::Up {
        return VerticalDirection::Down;
    }
    VerticalDirection::Up
}
