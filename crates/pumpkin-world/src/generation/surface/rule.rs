use pumpkin_data::{
    BlockState,
    material_rule::{ConditionMaterialRule, MaterialRule, SequenceMaterialRule},
};

use super::MaterialRuleContext;
use crate::{
    ProtoChunk,
    generation::{
        noise::router::surface_height_sampler::SurfaceHeightEstimateSampler,
        surface::test_condition,
    },
};

pub fn try_apply_material_rule(
    rule: &MaterialRule,
    chunk: &mut ProtoChunk,
    context: &mut MaterialRuleContext,
    surface_height_estimate_sampler: &mut SurfaceHeightEstimateSampler,
) -> Option<&'static BlockState> {
    match rule {
        MaterialRule::Badlands(_badlands) => Some(BadLandsMaterialRule::try_apply(context)),
        MaterialRule::Block(block) => Some(BlockMaterialRule::try_apply(block.result_state)),
        MaterialRule::Sequence(sequence) => {
            try_apply_sequence(sequence, chunk, context, surface_height_estimate_sampler)
        }
        MaterialRule::Condition(condition) => {
            try_apply_condition(condition, chunk, context, surface_height_estimate_sampler)
        }
        MaterialRule::OreVein(ore_vein) => context.try_apply_ore_vein(ore_vein),
    }
}

pub struct BadLandsMaterialRule;

impl BadLandsMaterialRule {
    pub fn try_apply(context: &mut MaterialRuleContext) -> &'static BlockState {
        context.terrain_builder.get_terracotta_block(
            context.block_pos_x,
            context.block_pos_y,
            context.block_pos_z,
        )
    }
}

pub struct BlockMaterialRule;

impl BlockMaterialRule {
    pub const fn try_apply(state: &'static BlockState) -> &'static BlockState {
        state
    }
}

pub fn try_apply_sequence(
    rule: &SequenceMaterialRule,
    chunk: &mut ProtoChunk,
    context: &mut MaterialRuleContext,
    surface_height_estimate_sampler: &mut SurfaceHeightEstimateSampler,
) -> Option<&'static BlockState> {
    for seq in rule.sequence {
        // Most entries are `condition -> block`; resolving those here saves two calls each.
        let state = match seq {
            MaterialRule::Condition(condition) => {
                if !test_condition(
                    &condition.if_true,
                    chunk,
                    context,
                    surface_height_estimate_sampler,
                ) {
                    continue;
                }
                match condition.then_run {
                    MaterialRule::Block(block) => Some(block.result_state),
                    then_run => try_apply_material_rule(
                        then_run,
                        chunk,
                        context,
                        surface_height_estimate_sampler,
                    ),
                }
            }
            _ => try_apply_material_rule(seq, chunk, context, surface_height_estimate_sampler),
        };
        if state.is_some() {
            return state;
        }
    }
    None
}

pub fn try_apply_condition(
    rule: &ConditionMaterialRule,
    chunk: &mut ProtoChunk,
    context: &mut MaterialRuleContext,
    surface_height_estimate_sampler: &mut SurfaceHeightEstimateSampler,
) -> Option<&'static BlockState> {
    if test_condition(
        &rule.if_true,
        chunk,
        context,
        surface_height_estimate_sampler,
    ) {
        return try_apply_material_rule(
            rule.then_run,
            chunk,
            context,
            surface_height_estimate_sampler,
        );
    }
    None
}
