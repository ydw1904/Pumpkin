use std::sync::Arc;

use crate::block::blocks::campfire::CampfireBlock;
use crate::block::entities::beehive::BeehiveBlockEntity;
use crate::block::registry::BlockActionResult;
use crate::block::{
    BlockBehaviour, BlockMetadata, GetComparatorOutputArgs, OnPlaceArgs, UseWithItemArgs,
};
use crate::entity::EntityBase;
use crate::entity::mob::Mob;
use crate::entity::passive::bee::BeeEntity;
use crate::entity::player::Player;
use crate::plugin::api::events::entity::entity_target::TargetReason;
use crate::world::World;
use pumpkin_data::block_properties::BeeNestLikeProperties;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::statistic::StatisticCategory;
use pumpkin_data::{BlockId, BlockStateId};
use pumpkin_util::GameMode;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::random::gameplay::game_random;
use pumpkin_world::world::BlockFlags;

/// Vanilla `BeehiveBlock.MAX_HONEY_LEVELS`.
const MAX_HONEY_LEVELS: u8 = 5;
/// The honeycombs the `harvest/beehive` loot table drops.
const HARVESTED_HONEYCOMBS: u8 = 3;

pub struct BeehiveBlock;

impl BeehiveBlock {
    /// Vanilla `releaseBeesAndResetHoneyLevel`. A player harvesting the hive makes it an
    /// `EMERGENCY` release whose bees go for the player; a dispenser's shears release with
    /// `BEE_RELEASED`.
    pub fn release_bees_and_reset_honey_level(
        world: &Arc<World>,
        pos: &BlockPos,
        player: Option<&Arc<Player>>,
    ) {
        let (block, state_id) = world.get_block_and_state_id(pos);
        let mut props = BeeNestLikeProperties::from_state_id(state_id);
        let facing = props.facing;
        props.honey_level = 0;
        world.set_block_state(pos, props.to_state_id(block), BlockFlags::NOTIFY_ALL);
        if let Some(block_entity) = world.get_block_entity(pos)
            && let Some(beehive) = block_entity.as_any().downcast_ref::<BeehiveBlockEntity>()
        {
            beehive.empty_all_living_from_hive(world, facing, player, player.is_some());
        }
    }

    /// Vanilla `resetHoneyLevel`.
    fn reset_honey_level(world: &Arc<World>, pos: &BlockPos) {
        let (block, state_id) = world.get_block_and_state_id(pos);
        let mut props = BeeNestLikeProperties::from_state_id(state_id);
        props.honey_level = 0;
        world.set_block_state(pos, props.to_state_id(block), BlockFlags::NOTIFY_ALL);
    }

    /// Vanilla `hiveContainsBees`.
    fn hive_contains_bees(world: &World, pos: &BlockPos) -> bool {
        world.get_block_entity(pos).is_some_and(|block_entity| {
            block_entity
                .as_any()
                .downcast_ref::<BeehiveBlockEntity>()
                .is_some_and(|beehive| !beehive.is_empty())
        })
    }

    /// Vanilla `angerNearbyBees`: every bee without a target near the hive picks one of the
    /// players there.
    fn anger_nearby_bees(world: &Arc<World>, pos: &BlockPos) {
        let area = BoundingBox::from_block(pos).expand(8.0, 6.0, 8.0);
        let bees = world
            .get_entities_at_box(&area)
            .into_iter()
            .filter(|entity| entity.cast_any().is::<BeeEntity>())
            .collect::<Vec<_>>();
        if bees.is_empty() {
            return;
        }
        let players = world
            .get_players_at_box(&area)
            .into_iter()
            .filter(|player| player.gamemode.load() != GameMode::Spectator)
            .collect::<Vec<_>>();
        if players.is_empty() {
            return;
        }
        for bee in bees {
            let Some(bee) = bee.cast_any().downcast_ref::<BeeEntity>() else {
                continue;
            };
            if bee.mob_entity.get_target().is_none() {
                let target = players[game_random().random_range(0..players.len())].clone();
                bee.set_mob_target(Some(target), TargetReason::ClosestPlayer);
            }
        }
    }

    /// The shears part of vanilla `useItemOn`. `false`: a plugin cancelled the harvest.
    fn shear_honeycomb(args: &mut UseWithItemArgs<'_>) -> bool {
        let player = args.player;
        let mut drops = vec![ItemStack::new(HARVESTED_HONEYCOMBS, &Item::HONEYCOMB)];
        if let Some(server) = args.world.server.upgrade() {
            let mut event =
                crate::plugin::api::events::player::player_harvest_block::PlayerHarvestBlockEvent {
                    player: player.clone(),
                    block_pos: *args.position,
                    harvested_items: drops.clone(),
                    cancelled: false,
                };
            server.plugin_manager.fire_blocking(&server, &mut event);
            if event.cancelled {
                return false;
            }
            drops = event.harvested_items;
        }
        for drop in drops {
            args.world.drop_stack(args.position, drop);
        }
        args.world.play_sound(
            Sound::BlockBeehiveShear,
            SoundCategory::Blocks,
            &player.position(),
        );
        let _ = player.damage_item_stack(args.item_stack, 1);
        args.world.emit_game_event_with_context(
            GameEvent::Shear.name(),
            args.position.to_f64(),
            &crate::world::game_event::GameEventContext {
                source_entity: Some(player.as_ref()),
            },
        );
        true
    }

    /// The glass bottle part of vanilla `useItemOn`.
    fn fill_honey_bottle(args: &mut UseWithItemArgs<'_>) {
        let player = args.player;
        let gamemode = player.gamemode.load();
        // Vanilla plays it for everyone but the player, whose client predicts it.
        args.world.play_sound_expect(
            player,
            Sound::ItemBottleFill,
            SoundCategory::Blocks,
            &player.position(),
        );
        let mut honey_bottle = ItemStack::new(1, &Item::HONEY_BOTTLE);
        if args.item_stack.item_count == 1 && gamemode != GameMode::Creative {
            *args.item_stack = honey_bottle;
        } else {
            args.item_stack.decrement_unless_creative(gamemode, 1);
            if !player.inventory().insert_stack_anywhere(&mut honey_bottle)
                && !honey_bottle.is_empty()
            {
                args.world
                    .drop_stack(&player.position().to_block_pos(), honey_bottle);
            }
        }
        args.world.emit_game_event_with_context(
            GameEvent::FluidPickup.name(),
            args.position.to_f64(),
            &crate::world::game_event::GameEventContext {
                source_entity: Some(player.as_ref()),
            },
        );
    }
}

impl BlockMetadata for BeehiveBlock {
    fn ids() -> Box<[BlockId]> {
        [BlockId::BEEHIVE, BlockId::BEE_NEST].into()
    }
}

impl BlockBehaviour for BeehiveBlock {
    fn on_place(&self, args: OnPlaceArgs<'_>) -> BlockStateId {
        let mut props = BeeNestLikeProperties::default(args.block);
        props.facing = args.player.get_entity().get_horizontal_facing().opposite();
        props.to_state_id(args.block)
    }

    /// Vanilla `BeehiveBlock.useItemOn`: shears or a glass bottle empty a full hive, which
    /// releases and angers its bees unless a campfire smokes them.
    fn use_with_item(&self, mut args: UseWithItemArgs<'_>) -> BlockActionResult {
        let state_id = args.world.get_block_state_id(args.position);
        if BeeNestLikeProperties::from_state_id(state_id).honey_level < MAX_HONEY_LEVELS {
            return BlockActionResult::PassToDefaultBlockAction;
        }
        let item = args.item_stack.item;
        let hive_emptied = if item == &Item::SHEARS {
            Self::shear_honeycomb(&mut args)
        } else if item == &Item::GLASS_BOTTLE {
            Self::fill_honey_bottle(&mut args);
            true
        } else {
            false
        };
        if !hive_emptied {
            return BlockActionResult::PassToDefaultBlockAction;
        }
        if CampfireBlock::is_smokey_pos(args.world, args.position) {
            Self::reset_honey_level(args.world, args.position);
        } else {
            if Self::hive_contains_bees(args.world, args.position) {
                Self::anger_nearby_bees(args.world, args.position);
            }
            Self::release_bees_and_reset_honey_level(args.world, args.position, Some(args.player));
        }
        args.player
            .increment_stat(StatisticCategory::Used, i32::from(item.id), 1);
        BlockActionResult::Success
    }

    fn get_comparator_output(&self, args: GetComparatorOutputArgs<'_>) -> Option<u8> {
        let state_id = args.world.get_block_state_id(args.position);
        let props = BeeNestLikeProperties::from_state_id(state_id);
        Some(props.honey_level)
    }
}
