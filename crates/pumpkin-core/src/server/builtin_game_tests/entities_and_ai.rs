use pumpkin_data::Block;
use pumpkin_gametest::{GameTestHelper, GameTestResult};
use pumpkin_util::math::{position::BlockPos, vector3::Vector3};

use super::build_stone_floor;

pub(super) async fn test_item_entities_merge(helper: GameTestHelper) -> GameTestResult<()> {
    build_stone_floor(&helper, 4, 4).await?;

    for x in 0..4 {
        for z in 0..4 {
            if x == 0 || x == 3 || z == 0 || z == 3 {
                helper
                    .set_block_default(&BlockPos::new(x, 1, z), &Block::STONE)
                    .await?;
            }
        }
    }

    helper
        .spawn_item_at("minecraft:diamond", 10, Vector3::new(1.5, 1.1, 1.5))
        .await?;
    helper
        .spawn_item_at("minecraft:diamond", 15, Vector3::new(1.5, 1.1, 1.5))
        .await?;

    let center = BlockPos::new(1, 1, 1);
    helper.succeed_when(move |helper| async move {
        helper.assert_entities_present("minecraft:item", 1).await?;
        helper
            .assert_item_entity_count("minecraft:diamond", &center, 2.0, 25)
            .await
    })?;
    Ok(())
}

pub(super) async fn test_zombie_pathfinds_to_villager(
    helper: GameTestHelper,
) -> GameTestResult<()> {
    build_stone_floor(&helper, 5, 8).await?;

    for x in 0..5 {
        for z in 0..8 {
            if x == 0 || x == 4 || z == 0 || z == 7 {
                helper
                    .set_block_default(&BlockPos::new(x, 1, z), &Block::STONE)
                    .await?;
                helper
                    .set_block_default(&BlockPos::new(x, 2, z), &Block::STONE)
                    .await?;
            }
        }
    }

    helper
        .spawn_entity_at("minecraft:villager", Vector3::new(2.5, 1.0, 6.0))
        .await?;
    helper
        .spawn_entity_at("minecraft:husk", Vector3::new(2.5, 1.0, 1.5))
        .await?;

    helper.succeed_when(move |helper| async move {
        helper
            .assert_entity_present_in_box(
                "minecraft:husk",
                BlockPos::new(1, 1, 4),
                BlockPos::new(4, 3, 7),
            )
            .await
    })?;
    Ok(())
}

pub(super) async fn test_minecart_drops_item_when_destroyed(
    helper: GameTestHelper,
) -> GameTestResult<()> {
    build_stone_floor(&helper, 5, 5).await?;
    helper
        .spawn_entity_at("minecraft:minecart", Vector3::new(2.5, 1.0, 2.5))
        .await?;
    let center = BlockPos::new(2, 1, 2);

    helper
        .start_sequence()
        .then_execute(move |helper| async move {
            helper
                .execute_command(
                    &center,
                    "damage @e[type=minecraft:minecart,distance=..2,limit=1] 50",
                )
                .await
                .map(|_| ())
        })
        .then_wait_until(move |helper| async move {
            helper
                .assert_entity_not_present("minecraft:minecart")
                .await?;
            helper
                .assert_item_entity_present("minecraft:minecart", &center, 1.0)
                .await
        })
        .then_succeed();
    Ok(())
}

/// A bee carrying nectar finds the hive beside it through its POI and goes in to make honey.
pub(super) async fn test_bee_with_nectar_enters_hive(helper: GameTestHelper) -> GameTestResult<()> {
    build_stone_floor(&helper, 5, 5).await?;
    let hive = BlockPos::new(2, 1, 2);
    helper.set_block_default(&hive, &Block::BEEHIVE).await?;
    helper
        .execute_command(
            &BlockPos::new(2, 1, 3),
            "summon minecraft:bee ~ ~ ~ {HasNectar:1b}",
        )
        .await?;

    helper.succeed_when(move |helper| async move {
        helper.assert_entity_not_present("minecraft:bee").await?;
        let bees = helper
            .block_entity_nbt(&hive)
            .await?
            .and_then(|nbt| nbt.get_list("bees").map(<[_]>::to_vec))
            .unwrap_or_default();
        let min_ticks = bees
            .first()
            .and_then(|bee| bee.extract_compound())
            .and_then(|bee| bee.get_int("min_ticks_in_hive"));
        helper.assert_true(
            bees.len() == 1 && min_ticks == Some(2400),
            format!(
                "Expected one bee staying 2400 ticks, got {} bees with {min_ticks:?}",
                bees.len()
            ),
        )
    })
}

/// A creaking bound to a heart that doesn't count it as its protector dies at once, like vanilla
/// `Creaking.tick`, instead of staying invulnerable.
pub(super) async fn test_creaking_without_protecting_heart_dies(
    helper: GameTestHelper,
) -> GameTestResult<()> {
    build_stone_floor(&helper, 5, 5).await?;
    let heart = BlockPos::new(2, 1, 2);
    helper
        .set_block_default(&heart, &Block::CREAKING_HEART)
        .await?;
    helper.assert_true(
        helper.block_entity_nbt(&heart).await?.is_some(),
        "Expected the creaking heart's block entity",
    )?;
    let home = helper.absolute_vec(Vector3::new(2.0, 1.0, 2.0));
    helper
        .execute_command(
            &BlockPos::new(2, 1, 4),
            &format!(
                "summon minecraft:creaking ~ ~ ~ {{home_pos:[I;{},{},{}]}}",
                home.x as i32, home.y as i32, home.z as i32
            ),
        )
        .await?;
    helper.assert_entity_present("minecraft:creaking").await?;

    helper.succeed_when(move |helper| async move {
        helper.assert_entity_not_present("minecraft:creaking").await
    })
}

pub(super) async fn test_animal_does_not_breed_with_partner_out_of_love(
    helper: GameTestHelper,
) -> GameTestResult<()> {
    build_stone_floor(&helper, 5, 5).await?;
    helper
        .execute_command(
            &BlockPos::new(1, 1, 2),
            "summon minecraft:armadillo ~0.5 ~ ~0.5 {InLove:600,PersistenceRequired:1b}",
        )
        .await?;
    helper
        .execute_command(
            &BlockPos::new(3, 1, 2),
            "summon minecraft:armadillo ~0.5 ~ ~0.5 {PersistenceRequired:1b}",
        )
        .await?;

    // Vanilla `Animal.canMate` needs both animals in love, so `AnimalMakeLove` never starts.
    helper
        .start_sequence()
        .then_execute_for(250, |helper| async move {
            helper
                .assert_entities_present("minecraft:armadillo", 2)
                .await
        })
        .then_succeed();
    Ok(())
}
