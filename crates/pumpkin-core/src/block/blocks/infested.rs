use std::sync::Arc;

use pumpkin_data::{
    Block, BlockId, BlockState, BlockStateId, Enchantment, Rotation,
    block_properties::{Axis, PaleOakWoodLikeProperties},
    entity::{EntityStatus, EntityType},
};
use pumpkin_protocol::bedrock::server::ActorEventID;
use pumpkin_util::{
    GameMode,
    math::{position::BlockPos, vector3::Vector3},
};
use uuid::Uuid;

use crate::{
    block::{BlockBehaviour, BlockMetadata, BrokenArgs, OnPlaceArgs},
    entity::r#type::from_type,
    world::World,
};

pub struct InfestedBlock;

/// Vanilla `InfestedBlock.BLOCK_BY_HOST_BLOCK`.
const HOST_TO_INFESTED: &[(&Block, &Block)] = &[
    (&Block::STONE, &Block::INFESTED_STONE),
    (&Block::COBBLESTONE, &Block::INFESTED_COBBLESTONE),
    (&Block::STONE_BRICKS, &Block::INFESTED_STONE_BRICKS),
    (
        &Block::MOSSY_STONE_BRICKS,
        &Block::INFESTED_MOSSY_STONE_BRICKS,
    ),
    (
        &Block::CRACKED_STONE_BRICKS,
        &Block::INFESTED_CRACKED_STONE_BRICKS,
    ),
    (
        &Block::CHISELED_STONE_BRICKS,
        &Block::INFESTED_CHISELED_STONE_BRICKS,
    ),
    (&Block::DEEPSLATE, &Block::INFESTED_DEEPSLATE),
];

impl InfestedBlock {
    /// Vanilla `InfestedBlock.isCompatibleHostBlock`.
    #[must_use]
    pub fn is_compatible_host_block(state: &BlockState) -> bool {
        let block = state.id.to_block();
        HOST_TO_INFESTED.iter().any(|(host, _)| host.id == block.id)
    }

    /// Vanilla `InfestedBlock.infestedStateByHost`.
    #[must_use]
    pub fn infested_state_by_host(host_state: &BlockState) -> Option<BlockStateId> {
        let block = host_state.id.to_block();
        let (_, infested) = HOST_TO_INFESTED
            .iter()
            .find(|(host, _)| host.id == block.id)?;
        Some(copy_properties(host_state, infested))
    }

    /// Vanilla `InfestedBlock.hostStateByInfested`.
    #[must_use]
    pub fn host_state_by_infested(infested_state: &BlockState) -> Option<BlockStateId> {
        let block = infested_state.id.to_block();
        let (host, _) = HOST_TO_INFESTED
            .iter()
            .find(|(_, infested)| infested.id == block.id)?;
        Some(copy_properties(infested_state, host))
    }

    /// Vanilla `InfestedBlock.spawnInfestation`.
    pub fn spawn_infestation(world: &Arc<World>, position: &BlockPos) {
        let silverfish = from_type(
            &EntityType::SILVERFISH,
            Vector3::new(
                f64::from(position.0.x) + 0.5,
                f64::from(position.0.y),
                f64::from(position.0.z) + 0.5,
            ),
            world,
            Uuid::new_v4(),
        );
        world.spawn_entity(silverfish.clone());
        world.send_entity_status(
            silverfish.get_entity(),
            EntityStatus::SilverfishMergeAnim,
            ActorEventID::SilverfishMergeAnimation,
        );
    }
}

/// Vanilla `InfestedBlock.getNewStateWithProperties`.
fn copy_properties(from: &BlockState, to: &'static Block) -> BlockStateId {
    from.id
        .to_block()
        .properties(from.id)
        .map_or(to.default_state.id, |properties| {
            to.from_properties(&properties.to_props()).to_state_id(to)
        })
}

impl BlockMetadata for InfestedBlock {
    fn ids() -> Box<[BlockId]> {
        [
            BlockId::INFESTED_STONE,
            BlockId::INFESTED_COBBLESTONE,
            BlockId::INFESTED_STONE_BRICKS,
            BlockId::INFESTED_MOSSY_STONE_BRICKS,
            BlockId::INFESTED_CRACKED_STONE_BRICKS,
            BlockId::INFESTED_CHISELED_STONE_BRICKS,
            BlockId::INFESTED_DEEPSLATE,
        ]
        .into()
    }
}

impl BlockBehaviour for InfestedBlock {
    fn on_place(&self, args: OnPlaceArgs<'_>) -> BlockStateId {
        if args.block == &Block::INFESTED_DEEPSLATE {
            let mut props = PaleOakWoodLikeProperties::default(args.block);
            props.axis = args.direction.to_axis();
            props.to_state_id(args.block)
        } else {
            args.block.default_state.id
        }
    }

    fn rotate(
        &self,
        block: &Block,
        state_id: BlockStateId,
        rotation: Rotation,
    ) -> &'static BlockState {
        if block == &Block::INFESTED_DEEPSLATE {
            let mut props = PaleOakWoodLikeProperties::from_state_id(state_id);
            if matches!(
                rotation,
                Rotation::Clockwise90 | Rotation::CounterClockwise90
            ) {
                props.axis = match props.axis {
                    Axis::X => Axis::Z,
                    Axis::Z => Axis::X,
                    Axis::Y => Axis::Y,
                };
            }
            BlockState::from_id(props.to_state_id(block))
        } else {
            BlockState::from_id(state_id)
        }
    }

    fn broken(&self, args: BrokenArgs<'_>) {
        // TODO: ugly fix, use onStacksDropped
        if args.player.gamemode.load() == GameMode::Creative
            || !args.world.level_info.load().game_rules.block_drops
            || args
                .player
                .inventory()
                .held_item()
                .get_enchantment_level(&Enchantment::SILK_TOUCH)
                > 0
        {
            return;
        }
        Self::spawn_infestation(args.world, args.position);
    }
}
