use std::sync::Arc;

use crate::block::entities::bed::BedBlockEntity;
use pumpkin_data::block_properties::BedPart;
use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::EntityType;
use pumpkin_data::translation;
use pumpkin_data::{Block, BlockState, BlockStateId};
use pumpkin_macros::pumpkin_block_from_tag;
use pumpkin_util::GameMode;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::text::TextComponent;
use pumpkin_world::world::BlockFlags;

use crate::block::OnLandedUponArgs;
use crate::block::UpdateEntityMovementAfterFallOnArgs;
use crate::block::bounce_entity_after_fall;
use crate::block::registry::BlockActionResult;
use crate::block::{
    BlockBehaviour, BrokenArgs, CanPlaceAtArgs, GetStateForNeighborUpdateArgs, NormalUseArgs,
    OnPlaceArgs, OnStateReplacedArgs, PathComputationType, PlacedArgs,
};
use crate::entity::{
    EntityBase,
    mob::{Mob, is_monster},
    player::Player,
};
use crate::world::World;
use pumpkin_data::HorizontalFacingExt;

type BedProperties = pumpkin_data::block_properties::WhiteBedLikeProperties;

/// The half extents of vanilla `ServerPlayer.startSleepInBed`'s monster box.
const MONSTER_H_RANGE: f64 = 8.0;
const MONSTER_V_RANGE: f64 = 5.0;

#[pumpkin_block_from_tag("minecraft:beds")]
pub struct BedBlock;

impl BlockBehaviour for BedBlock {
    fn can_place_at(&self, args: CanPlaceAtArgs<'_>) -> bool {
        if let Some(player) = args.player {
            let facing = player.get_entity().get_horizontal_facing();
            return args
                .block_accessor
                .get_block_state(args.position)
                .replaceable()
                && args
                    .block_accessor
                    .get_block_state(&args.position.offset(facing.to_offset()))
                    .replaceable();
        }
        true
    }

    fn on_landed_upon(&self, args: OnLandedUponArgs<'_>) {
        if let Some(living) = args.entity.get_living_entity() {
            living.handle_fall_damage(args.entity, args.fall_distance * 0.5, 1.0);
        }
    }

    fn update_entity_movement_after_fall_on(&self, args: UpdateEntityMovementAfterFallOnArgs<'_>) {
        bounce_entity_after_fall(args.entity, 0.66);
    }

    fn on_place(&self, args: OnPlaceArgs<'_>) -> BlockStateId {
        let mut bed_props = BedProperties::default(args.block);

        bed_props.facing = args.player.get_entity().get_horizontal_facing();
        bed_props.part = BedPart::Foot;

        bed_props.to_state_id(args.block)
    }

    fn placed(&self, args: PlacedArgs<'_>) {
        {
            let bed_entity = BedBlockEntity::new(*args.position);
            args.world.add_block_entity(Arc::new(bed_entity));

            let mut bed_head_props = BedProperties::default(args.block);
            bed_head_props.facing = BedProperties::from_state_id(args.state_id).facing;
            bed_head_props.part = BedPart::Head;

            let bed_head_pos = args.position.offset(bed_head_props.facing.to_offset());
            args.world.set_block_state(
                &bed_head_pos,
                bed_head_props.to_state_id(args.block),
                BlockFlags::NOTIFY_ALL | BlockFlags::SKIP_BLOCK_ADDED_CALLBACK,
            );

            let bed_head_entity = BedBlockEntity::new(bed_head_pos);
            args.world.add_block_entity(Arc::new(bed_head_entity));
        }
    }

    fn broken(&self, args: BrokenArgs<'_>) {
        let bed_props = BedProperties::from_state_id(args.state.id);
        let other_half_pos = if bed_props.part == BedPart::Head {
            args.position
                .offset(bed_props.facing.opposite().to_offset())
        } else {
            args.position.offset(bed_props.facing.to_offset())
        };
        let neighbor_state_id = args.world.get_block_state_id(&other_half_pos);
        if neighbor_state_id.to_block_id() != args.block.id {
            args.world.update_neighbors(&other_half_pos, None);
            return;
        }

        let is_creative = args.player.gamemode.load() == GameMode::Creative;
        let flags = if bed_props.part == BedPart::Foot && !is_creative {
            // Breaking foot in survival -> allow head to drop
            BlockFlags::NOTIFY_ALL
        } else {
            // Breaking head OR creative mode -> skip drops
            BlockFlags::SKIP_DROPS | BlockFlags::NOTIFY_ALL
        };

        args.world
            .break_block(&other_half_pos, Some(args.player), flags);
    }

    fn on_state_replaced(&self, args: OnStateReplacedArgs<'_>) {
        if args.moved {
            return;
        }

        let bed_props = BedProperties::from_state_id(args.old_state_id);
        let other_half_pos = if bed_props.part == BedPart::Head {
            args.position
                .offset(bed_props.facing.opposite().to_offset())
        } else {
            args.position.offset(bed_props.facing.to_offset())
        };

        let (other_block, other_state) = args.world.get_block_and_state(&other_half_pos);
        if other_block == args.block {
            let other_props = BedProperties::from_state_id(other_state.id);
            if other_props.part != bed_props.part {
                args.world.break_block(
                    &other_half_pos,
                    None,
                    BlockFlags::SKIP_DROPS | BlockFlags::NOTIFY_ALL,
                );
            }
        }
    }

    fn get_state_for_neighbor_update(
        &self,
        args: GetStateForNeighborUpdateArgs<'_>,
    ) -> BlockStateId {
        let mut bed_props = BedProperties::from_state_id(args.state_id);
        let neighbor_dir = if bed_props.part == BedPart::Foot {
            bed_props.facing.to_block_direction()
        } else {
            bed_props.facing.opposite().to_block_direction()
        };
        if args.direction == neighbor_dir {
            let neighbor_block = args.neighbor_state_id.to_block();
            if neighbor_block == args.block {
                let neighbor_props = BedProperties::from_state_id(args.neighbor_state_id);
                if neighbor_props.part != bed_props.part {
                    bed_props.occupied = neighbor_props.occupied;
                    return bed_props.to_state_id(args.block);
                }
            }
            return Block::AIR.default_state.id;
        }
        args.state_id
    }

    fn normal_use(&self, args: NormalUseArgs<'_>) -> BlockActionResult {
        Self::use_bed(args.world, args.player, args.block, args.position)
    }

    fn is_pathfindable(&self, _state: &BlockState, _computation_type: PathComputationType) -> bool {
        false
    }
}

impl BedBlock {
    fn use_bed(
        world: &Arc<World>,
        player: &Arc<Player>,
        block: &Block,
        position: &BlockPos,
    ) -> BlockActionResult {
        let state_id = world.get_block_state_id(position);
        let bed_props = BedProperties::from_state_id(state_id);

        let (bed_head_pos, bed_foot_pos) = if bed_props.part == BedPart::Head {
            (
                *position,
                position.offset(bed_props.facing.opposite().to_offset()),
            )
        } else {
            (position.offset(bed_props.facing.to_offset()), *position)
        };

        // Explode if bed rule explodes (EnvironmentAttributes.BED_RULE)
        if world.dimension.bed_rule.explodes {
            world.remove_block(&bed_head_pos);
            if world.get_block(&bed_foot_pos) == block {
                world.remove_block(&bed_foot_pos);
            }

            world.explode_with_fire_and_damage_type(
                bed_head_pos.to_centered_f64(),
                5.0,
                crate::world::ExplosionInteraction::Block,
                None,
                DamageType::BAD_RESPAWN_POINT,
            );

            return BlockActionResult::SuccessServer;
        }

        // Make sure the bed is not occupied
        if bed_props.occupied {
            // TODO: Wake up villager

            player.send_system_message_raw(
                &pumpkin_macros::translate_cross!(
                    translation::java::BLOCK_MINECRAFT_BED_OCCUPIED,
                    translation::bedrock::TILE_BED_OCCUPIED
                ),
                true,
            );
            return BlockActionResult::SuccessServer;
        }

        let is_dark = world.is_dark_outside();
        let can_set_spawn = world.dimension.bed_rule.can_set_spawn(is_dark);
        if let Err(problem) = start_sleep_in_bed_checks(
            world,
            player,
            bed_head_pos,
            bed_foot_pos,
            world.dimension.bed_rule.can_sleep(is_dark),
            can_set_spawn,
        ) {
            if let Some(problem) = problem {
                player.send_system_message_raw(&problem, true);
            }
            return BlockActionResult::SuccessServer;
        }

        if let Some(server) = world.server.upgrade() {
            let mut event =
                crate::plugin::api::events::player::player_bed::PlayerBedEnterEvent::new(
                    player.clone(),
                    bed_head_pos,
                );
            server.plugin_manager.fire_blocking(&server, &mut event);
            if event.cancelled {
                return BlockActionResult::SuccessServer;
            }
        }

        player.sleep(bed_head_pos);
        player.increment_stat(
            pumpkin_data::statistic::StatisticCategory::Custom,
            pumpkin_data::statistic::CustomStatistic::SleepInBed as i32,
            1,
        );
        if can_set_spawn {
            player.trigger_advancement(
                crate::entity::player::advancement::trigger::AdvancementTrigger::SleptInBed,
            );
        }
        Self::set_occupied(true, world, block, position, state_id);

        BlockActionResult::SuccessServer
    }
}

impl BedBlock {
    pub fn set_occupied(
        occupied: bool,
        world: &Arc<World>,
        block: &Block,
        block_pos: &BlockPos,
        state_id: BlockStateId,
    ) {
        let mut bed_props = BedProperties::from_state_id(state_id);
        bed_props.occupied = occupied;
        world.set_block_state(
            block_pos,
            bed_props.to_state_id(block),
            BlockFlags::NOTIFY_LISTENERS,
        );

        let other_half_pos = if bed_props.part == BedPart::Head {
            block_pos.offset(bed_props.facing.opposite().to_offset())
        } else {
            block_pos.offset(bed_props.facing.to_offset())
        };
        bed_props.part = if bed_props.part == BedPart::Head {
            BedPart::Foot
        } else {
            BedPart::Head
        };
        world.set_block_state(
            &other_half_pos,
            bed_props.to_state_id(block),
            BlockFlags::NOTIFY_LISTENERS,
        );
    }
}

/// The checks of vanilla `ServerPlayer.startSleepInBed` before the player lies down, in
/// vanilla order. `Err` holds the overlay message to show, if any.
pub(crate) fn start_sleep_in_bed_checks(
    world: &World,
    player: &Player,
    bed_head_pos: BlockPos,
    bed_foot_pos: BlockPos,
    can_sleep: bool,
    can_set_spawn: bool,
) -> Result<(), Option<TextComponent>> {
    if player.is_sleeping() || !player.living_entity.is_alive() {
        return Err(None);
    }

    // `bedInRange`: within 3 blocks across and 2 up or down of either half's bottom centre.
    let player_pos = player.position();
    let reachable = |pos: BlockPos| player_pos.is_within_bounds(pos.to_f64(), 3.0, 2.0, 3.0);
    if !reachable(bed_head_pos) && !reachable(bed_foot_pos) {
        return Err(Some(pumpkin_macros::translate_cross!(
            translation::java::BLOCK_MINECRAFT_BED_TOO_FAR_AWAY,
            translation::bedrock::TILE_BED_TOOFAR
        )));
    }

    // `bedBlocked`: `Player.freeAt` above both halves.
    if world.get_block_state(&bed_head_pos.up()).is_suffocating()
        || world.get_block_state(&bed_foot_pos.up()).is_suffocating()
    {
        return Err(Some(pumpkin_macros::translate_cross!(
            translation::java::BLOCK_MINECRAFT_BED_OBSTRUCTED,
            translation::bedrock::TILE_BED_OBSTRUCTED
        )));
    }

    if can_set_spawn
        && player.set_respawn_point(
            world.dimension.clone(),
            bed_head_pos,
            player.get_entity().yaw.load(),
            player.get_entity().pitch.load(),
            false,
        )
    {
        player.send_system_message(&pumpkin_macros::translate_cross!(
            translation::java::BLOCK_MINECRAFT_SET_SPAWN,
            translation::bedrock::TILE_BED_RESPAWNSET
        ));
    }

    if !can_sleep {
        return Err(Some(pumpkin_macros::translate_cross!(
            translation::java::BLOCK_MINECRAFT_BED_NO_SLEEP,
            translation::bedrock::TILE_BED_NOSLEEP
        )));
    }

    if !player.is_creative() && monster_prevents_rest(world, player, bed_head_pos) {
        return Err(Some(pumpkin_macros::translate_cross!(
            translation::java::BLOCK_MINECRAFT_BED_NOT_SAFE,
            translation::bedrock::TILE_BED_NOTSAFE
        )));
    }
    Ok(())
}

/// A `Monster` whose box meets the 16x10x16 box around the bed head's bottom centre and
/// whose `isPreventingPlayerRest` holds.
fn monster_prevents_rest(world: &World, player: &Player, bed_head_pos: BlockPos) -> bool {
    let center = bed_head_pos.to_f64();
    let area = BoundingBox::new(
        center.add_raw(-MONSTER_H_RANGE, -MONSTER_V_RANGE, -MONSTER_H_RANGE),
        center.add_raw(MONSTER_H_RANGE, MONSTER_V_RANGE, MONSTER_H_RANGE),
    );
    world
        .get_entities_of_types_at_box(&area, is_monster)
        .iter()
        .any(|monster| is_preventing_player_rest(monster.as_ref(), player, world))
}

/// Vanilla `Monster.isPreventingPlayerRest`: a zombified piglin only while angry at `player`.
fn is_preventing_player_rest(monster: &dyn EntityBase, player: &Player, world: &World) -> bool {
    if monster.get_entity().entity_type == &EntityType::ZOMBIFIED_PIGLIN {
        return monster
            .get_mob()
            .and_then(Mob::as_neutral)
            .is_some_and(|neutral| neutral.is_angry_at(player, world));
    }
    true
}
