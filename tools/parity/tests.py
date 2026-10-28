"""Specs for the pumpkin-parity datapack, see build_pack.py.

The start test block sits at (0, 1, 0). Check delay lines run along x=0 towards +z, so keep
mechanics at x >= 1 and away from that column. Every test here has to pass on vanilla first;
`parity.py` reports a test that fails on vanilla as broken rather than as a Pumpkin bug.
"""

from build_pack import Test

WATER_CHANNEL_WALLS = [(x, 1, z) for x in range(2, 10) for z in (1, 3)] + [(2, 1, 2), (9, 1, 2)]


def water_channel(test):
    for pos in WATER_CHANNEL_WALLS:
        test.set(pos, "minecraft:glass")
    return test


def row_of_planks(test, x_from, x_to):
    for x in range(x_from, x_to + 1):
        test.set((x, 1, 0), "minecraft:oak_planks")
    return test


TESTS = [
    # Vanilla's start block stays powered for the rest of the test (TestBlockEntity.trigger
    # never schedules the tick that would reset it).
    Test("start_block_stays_powered")
    .set((1, 1, 0), "minecraft:redstone_lamp[lit=false]")
    .check_after(2, ((1, 1, 0), "minecraft:redstone_lamp[lit=true]", "lamp next to the start block still lit")),

    Test("redstone_wire_carries_signal")
    .set((1, 1, 0), "minecraft:redstone_wire[east=side,west=side,north=none,south=none,power=0]")
    .set((2, 1, 0), "minecraft:redstone_wire[east=side,west=side,north=none,south=none,power=0]")
    .set((3, 1, 0), "minecraft:redstone_wire[east=side,west=side,north=none,south=none,power=0]")
    .set((4, 1, 0), "minecraft:redstone_lamp[lit=false]")
    .check_after(
        1,
        ((1, 1, 0), "minecraft:redstone_wire[power=15]", "wire next to the source at power 15"),
        ((3, 1, 0), "minecraft:redstone_wire[power=13]", "third wire at power 13"),
        ((4, 1, 0), "minecraft:redstone_lamp[lit=true]", "lamp at the end of the wire lit"),
    ),

    Test("redstone_torch_inverts")
    .set((1, 1, 0), "minecraft:redstone_wire[east=side,west=side,north=none,south=none,power=0]")
    .set((2, 1, 0), "minecraft:stone")
    .set((3, 1, 0), "minecraft:redstone_wall_torch[facing=east,lit=true]")
    .check_after(1, ((3, 1, 0), "minecraft:redstone_wall_torch[lit=false]", "torch on the powered block off")),

    Test("repeater_passes_signal")
    .set((1, 1, 0), "minecraft:repeater[delay=1,facing=west,locked=false,powered=false]")
    .set((2, 1, 0), "minecraft:redstone_lamp[lit=false]")
    .check_after(
        1,
        ((1, 1, 0), "minecraft:repeater[powered=true]", "repeater powered"),
        ((2, 1, 0), "minecraft:redstone_lamp[lit=true]", "lamp after the repeater lit"),
    ),

    Test("observer_sees_lamp_light", max_ticks=40)
    .set((1, 1, 0), "minecraft:redstone_lamp[lit=false]")
    .set((2, 1, 0), "minecraft:observer[facing=west,powered=false]")
    .accept((3, 1, 0)),

    Test("piston_pushes_block")
    .set((1, 1, 0), "minecraft:piston[extended=false,facing=east]")
    .set((2, 1, 0), "minecraft:oak_planks")
    .set((3, 1, 0), "minecraft:air")
    .check_after(
        1,
        ((1, 1, 0), "minecraft:piston[extended=true]", "piston extended"),
        ((2, 1, 0), "minecraft:piston_head[facing=east,type=normal]", "piston head in front of the piston"),
        ((3, 1, 0), "minecraft:oak_planks", "planks pushed one block"),
    ),

    Test("sticky_piston_pushes_block")
    .set((1, 1, 0), "minecraft:sticky_piston[extended=false,facing=east]")
    .set((2, 1, 0), "minecraft:oak_planks")
    .set((3, 1, 0), "minecraft:air")
    .check_after(
        1,
        ((1, 1, 0), "minecraft:sticky_piston[extended=true]", "sticky piston extended"),
        ((2, 1, 0), "minecraft:piston_head[facing=east,type=sticky]", "sticky head in front of the piston"),
        ((3, 1, 0), "minecraft:oak_planks", "planks pushed one block"),
    ),

    # Vanilla's push limit is 12 blocks; a 13th stops the piston from extending at all.
    row_of_planks(Test("piston_push_limit"), 2, 14)
    .set((1, 1, 0), "minecraft:piston[extended=false,facing=east]")
    .set((15, 1, 0), "minecraft:air")
    .check_after(
        1,
        ((1, 1, 0), "minecraft:piston[extended=false]", "piston still retracted with 13 blocks in front"),
        ((2, 1, 0), "minecraft:oak_planks", "first block not moved"),
    ),

    Test("piston_cannot_push_obsidian")
    .set((1, 1, 0), "minecraft:piston[extended=false,facing=east]")
    .set((2, 1, 0), "minecraft:obsidian")
    .set((3, 1, 0), "minecraft:air")
    .check_after(
        1,
        ((1, 1, 0), "minecraft:piston[extended=false]", "piston still retracted"),
        ((2, 1, 0), "minecraft:obsidian", "obsidian not moved"),
    ),

    Test("iron_door_opens_with_redstone")
    .set((1, 1, 0), "minecraft:iron_door[facing=east,half=lower,hinge=left,open=false,powered=false]")
    .set((1, 2, 0), "minecraft:iron_door[facing=east,half=upper,hinge=left,open=false,powered=false]")
    .check_after(
        1,
        ((1, 1, 0), "minecraft:iron_door[open=true]", "lower door half open"),
        ((1, 2, 0), "minecraft:iron_door[open=true]", "upper door half open"),
    ),

    Test("iron_trapdoor_opens_with_redstone")
    .set((1, 1, 0), "minecraft:iron_trapdoor[facing=east,half=bottom,open=false,powered=false,waterlogged=false]")
    .check_after(1, ((1, 1, 0), "minecraft:iron_trapdoor[open=true]", "trapdoor open")),

    Test("fence_gate_opens_with_redstone")
    .set((1, 1, 0), "minecraft:oak_fence_gate[facing=east,in_wall=false,open=false,powered=false]")
    .check_after(1, ((1, 1, 0), "minecraft:oak_fence_gate[open=true]", "fence gate open")),

    # Flowing water loses one level per block and spreads every 5 ticks.
    water_channel(Test("water_spreads_along_channel"))
    .command((1, 1, 0), "setblock ~2 ~ ~2 minecraft:water")
    .check_after(
        3,
        ((3, 1, 2), "minecraft:water[level=0]", "water source where it was placed"),
        ((4, 1, 2), "minecraft:water[level=1]", "level 1 water one block away"),
        ((6, 1, 2), "minecraft:water[level=3]", "level 3 water three blocks away"),
    ),

    water_channel(Test("water_turns_lava_source_to_obsidian"))
    .set((5, 1, 2), "minecraft:lava[level=0]")
    .command((1, 1, 0), "setblock ~2 ~ ~2 minecraft:water")
    .check_after(3, ((5, 1, 2), "minecraft:obsidian", "obsidian where the lava source was")),

    Test("sand_falls_to_the_floor")
    .set((3, 4, 2), "minecraft:air")
    .command((1, 1, 0), "setblock ~2 ~3 ~2 minecraft:sand")
    .check_after(
        3,
        ((3, 1, 2), "minecraft:sand", "sand landed on the floor"),
        ((3, 4, 2), "minecraft:air", "nothing left where the sand was placed"),
    ),

    Test("hopper_fills_chest", max_ticks=40)
    .container((3, 2, 2), "minecraft:hopper[enabled=true,facing=down]", {0: ("minecraft:stone", 1)})
    .container((3, 1, 2), "minecraft:chest[facing=north,type=single,waterlogged=false]", {})
    .set((3, 1, 3), "minecraft:comparator[facing=north,mode=compare,powered=false]")
    .accept((3, 1, 4)),

    # The hopper under a furnace only pulls from the result slot, so the comparator turns on
    # once an ingot has been smelted (200 ticks).
    Test("furnace_smelts_into_hopper", max_ticks=260)
    .container(
        (3, 2, 2),
        "minecraft:furnace[facing=north,lit=false]",
        {0: ("minecraft:raw_iron", 1), 1: ("minecraft:coal", 1)},
    )
    .container((3, 1, 2), "minecraft:hopper[enabled=true,facing=east]", {})
    .set((3, 1, 3), "minecraft:comparator[facing=north,mode=compare,powered=false]")
    .accept((3, 1, 4)),

    Test("dispenser_places_water", max_ticks=40)
    .container((1, 1, 0), "minecraft:dispenser[facing=east,triggered=false]", {0: ("minecraft:water_bucket", 1)})
    .set((2, 1, 0), "minecraft:air")
    .set((2, 2, 0), "minecraft:observer[facing=down,powered=false]")
    .accept((2, 3, 0)),
]
