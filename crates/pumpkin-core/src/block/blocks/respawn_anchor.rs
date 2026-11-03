use std::sync::Arc;

use pumpkin_data::block_properties::RespawnAnchorLikeProperties;
use pumpkin_data::damage::DamageType;
use pumpkin_data::fluid::FluidState;
use pumpkin_data::item::Item;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::Taggable;
use pumpkin_data::{Block, BlockDirection, tag};
use pumpkin_data::{BlockState, translation};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::world::BlockFlags;

use crate::block::registry::BlockActionResult;
use crate::block::{
    BlockBehaviour, GetComparatorOutputArgs, NormalUseArgs, PathComputationType, UseWithItemArgs,
};
use crate::entity::EntityBase;
use crate::world::World;
use crate::world::explosion::{Explosion, ExplosionDamageCalculator};

/// Vanilla `RespawnAnchorBlock.MAX_CHARGES`.
const MAX_CHARGES: u8 = 4;

#[pumpkin_block("minecraft:respawn_anchor")]
pub struct RespawnAnchorBlock;

impl RespawnAnchorBlock {
    /// Vanilla `RespawnAnchorBlock.explode`.
    fn explode(world: &Arc<World>, pos: BlockPos) {
        world.remove_block(&pos);
        let any_water_neighbors = BlockDirection::horizontal()
            .iter()
            .any(|dir| is_water_that_would_flow(world, &pos.offset(dir.to_offset())));
        let in_water = any_water_neighbors
            || world
                .get_fluid(&pos.up())
                .has_tag(&tag::Fluid::MINECRAFT_WATER);
        world.explode_with_fire_and_damage_type(
            pos.to_centered_f64(),
            5.0,
            crate::world::ExplosionInteraction::Block,
            Some(Arc::new(RespawnAnchorDamageCalculator { pos, in_water })),
            DamageType::BAD_RESPAWN_POINT,
        );
    }
}

/// Vanilla `RespawnAnchorBlock.isWaterThatWouldFlow`.
fn is_water_that_would_flow(world: &World, pos: &BlockPos) -> bool {
    let (fluid, state) = world.get_fluid_and_fluid_state(pos);
    if !fluid.has_tag(&tag::Fluid::MINECRAFT_WATER) {
        false
    } else if state.is_source {
        true
    } else if state.level < 2 {
        false
    } else {
        !world
            .get_fluid(&pos.down())
            .has_tag(&tag::Fluid::MINECRAFT_WATER)
    }
}

/// The anchor's own position resists like water when water would flow into it, so the
/// blast spares the surroundings like vanilla.
struct RespawnAnchorDamageCalculator {
    pos: BlockPos,
    in_water: bool,
}

impl ExplosionDamageCalculator for RespawnAnchorDamageCalculator {
    fn get_block_explosion_resistance(
        &self,
        _explosion: &Explosion,
        _world: &World,
        pos: &BlockPos,
        block: &Block,
        fluid: &FluidState,
    ) -> Option<f32> {
        if *pos == self.pos && self.in_water {
            Some(Block::WATER.blast_resistance)
        } else if block.default_state.is_air() && fluid.is_empty {
            None
        } else {
            Some(fluid.blast_resistance.max(block.blast_resistance))
        }
    }
}

impl BlockBehaviour for RespawnAnchorBlock {
    fn use_with_item(&self, args: UseWithItemArgs<'_>) -> BlockActionResult {
        if args.item_stack.item.id != Item::GLOWSTONE.id {
            return BlockActionResult::PassToDefaultBlockAction;
        }

        let state_id = args.world.get_block_state_id(args.position);
        let mut props = RespawnAnchorLikeProperties::from_state_id(state_id);

        if props.charges >= MAX_CHARGES {
            return BlockActionResult::PassToDefaultBlockAction;
        }

        props.charges += 1;
        args.world.set_block_state(
            args.position,
            props.to_state_id(args.block),
            BlockFlags::NOTIFY_ALL,
        );
        args.world.emit_game_event(
            pumpkin_data::game_event::GameEvent::BlockChange.name(),
            args.position.to_centered_f64(),
        );

        args.item_stack
            .decrement_unless_creative(args.player.gamemode.load(), 1);

        args.world.play_sound(
            Sound::BlockRespawnAnchorCharge,
            SoundCategory::Blocks,
            &args.position.to_f64(),
        );

        BlockActionResult::Success
    }

    fn normal_use(&self, args: NormalUseArgs<'_>) -> BlockActionResult {
        let state_id = args.world.get_block_state_id(args.position);
        let props = RespawnAnchorLikeProperties::from_state_id(state_id);

        if props.charges == 0 {
            return BlockActionResult::Pass;
        }

        if !args.world.dimension.respawn_anchor_works {
            Self::explode(args.world, *args.position);
            return BlockActionResult::SuccessServer;
        }

        let player = args.player;
        let world = args.world;
        let pos = *args.position;
        if player.set_respawn_point(
            world.dimension.clone(),
            pos,
            player.get_entity().yaw.load(),
            player.get_entity().pitch.load(),
            false,
        ) {
            world.play_sound(
                Sound::BlockRespawnAnchorSetSpawn,
                SoundCategory::Blocks,
                &pos.to_f64(),
            );

            player.send_system_message(&pumpkin_macros::translate_cross!(
                translation::java::BLOCK_MINECRAFT_SET_SPAWN,
                translation::bedrock::TILE_BED_RESPAWNSET
            ));
            return BlockActionResult::SuccessServer;
        }

        BlockActionResult::Consume
    }

    /// Charges scale over the full signal range, so each charge is worth 15 / 4.
    fn get_comparator_output(&self, args: GetComparatorOutputArgs<'_>) -> Option<u8> {
        let props = RespawnAnchorLikeProperties::from_state_id(args.state.id);
        Some(props.charges * 15 / MAX_CHARGES)
    }

    fn is_pathfindable(&self, _state: &BlockState, _computation_type: PathComputationType) -> bool {
        false
    }
}
