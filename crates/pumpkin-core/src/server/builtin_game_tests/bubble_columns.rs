use pumpkin_data::{Block, block_properties::BubbleColumnLikeProperties};
use pumpkin_gametest::{GameTestHelper, GameTestResult};
use pumpkin_util::math::position::BlockPos;

use super::build_stone_floor;

pub(super) async fn test_bubble_column_upward_and_downward(
    helper: GameTestHelper,
) -> GameTestResult<()> {
    build_stone_floor(&helper, 5, 5).await?;

    // Two one-block shafts, three blocks deep, above soul sand and magma.
    for y in 1..=3 {
        for x in 0..5 {
            for z in 0..5 {
                if !((x == 1 || x == 3) && z == 2) {
                    helper
                        .set_block_default(&BlockPos::new(x, y, z), &Block::STONE)
                        .await?;
                }
            }
        }
    }

    for y in 1..=3 {
        helper
            .set_block_default(&BlockPos::new(1, y, 2), &Block::WATER)
            .await?;
        helper
            .set_block_default(&BlockPos::new(3, y, 2), &Block::WATER)
            .await?;
    }
    let soul_sand_floor = BlockPos::new(1, 0, 2);
    let magma_floor = BlockPos::new(3, 0, 2);

    let expected_upward =
        BubbleColumnLikeProperties { drag: false }.to_state_id(&Block::BUBBLE_COLUMN);
    let expected_downward =
        BubbleColumnLikeProperties { drag: true }.to_state_id(&Block::BUBBLE_COLUMN);
    let water = Block::WATER.default_state.id;

    // Vanilla 26.3 fills the whole column in one tick, 20 ticks after its support is placed,
    // and removes it 5 ticks after its support goes.
    helper
        .start_sequence()
        // The supports go in under water that has settled, so only the bottom block hears it.
        .then_idle(25)
        .then_execute(move |helper| async move {
            helper
                .set_block_default(&soul_sand_floor, &Block::SOUL_SAND)
                .await?;
            helper
                .set_block_default(&magma_floor, &Block::MAGMA_BLOCK)
                .await
        })
        .then_idle(22)
        .then_execute(move |helper| async move {
            for y in 1..=3 {
                helper
                    .assert_block_state(&BlockPos::new(1, y, 2), expected_upward)
                    .await?;
                helper
                    .assert_block_state(&BlockPos::new(3, y, 2), expected_downward)
                    .await?;
            }
            helper.set_block_default(&magma_floor, &Block::STONE).await
        })
        .then_idle(7)
        .then_execute(move |helper| async move {
            for y in 1..=3 {
                helper
                    .assert_block_state(&BlockPos::new(1, y, 2), expected_upward)
                    .await?;
                helper
                    .assert_block_state(&BlockPos::new(3, y, 2), water)
                    .await?;
            }
            Ok(())
        })
        .then_succeed();
    Ok(())
}
