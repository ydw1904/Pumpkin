// Tick trace: block writes whose vanilla update flags decide what neighbours and clients see.
// - target and sculk sensor placed with power unpower in `onPlace` with flags 18, and the outer
//   `setBlock` then skips its shape updates, so the wire next to them keeps its shape;
// - a powered lightning rod stays powered until its tick, a shrieking shrieker keeps shrieking;
// - fire ageing and a sapling's stage change use `UPDATE_NONE`, so clients get no packet;
// - waxing one copper door half converts the other through its shape update, without drops.
const { trace, PLATFORM_Y } = require('./steptrace')
const Y = PLATFORM_Y + 1

trace({
  name: 'block-flags',
  setup: [
    `/setblock -10 ${Y} -10 redstone_wire`,
    `/setblock -9 ${Y} -10 target[power=5]`,
    `/setblock -10 ${Y} -7 redstone_wire`,
    `/setblock -9 ${Y} -7 sculk_sensor[power=5]`,
    `/setblock -10 ${Y} -4 redstone_wire`,
    `/setblock -9 ${Y} -4 lightning_rod[powered=true]`,
    `/setblock -9 ${Y} -1 sculk_shrieker[shrieking=true]`,
    `/setblock -9 ${Y} 5 netherrack`,
    `/setblock -9 ${Y + 1} 5 fire`,
    // Strict, so neither server's placement callbacks add or reshape a half.
    `/setblock -4 ${Y} -10 copper_door[half=lower,facing=north] strict`,
    `/setblock -4 ${Y + 1} -10 copper_door[half=upper,facing=north] strict`,
    `/setblock -4 ${Y} -11 dispenser[facing=south]{Items:[{Slot:0b,id:"minecraft:honeycomb",count:4}]}`,
    `/setblock -4 ${Y} -12 redstone_block`,
    `/setblock 6 ${PLATFORM_Y} -10 dirt`,
    `/setblock 6 ${Y} -10 oak_sapling`,
    `/setblock 6 ${Y} -11 dispenser[facing=south]{Items:[{Slot:0b,id:"minecraft:bone_meal",count:1}]}`,
    `/setblock 6 ${Y} -12 redstone_block`
  ],
  probes: {
    target_wire: { block: [-10, Y, -10] },
    sensor_wire: { block: [-10, Y, -7] },
    rod_wire: { block: [-10, Y, -4] },
    rod: { server: [-9, Y, -4], states: ['lightning_rod[powered=true]', 'lightning_rod[powered=false]'] },
    shrieker: { server: [-9, Y, -1], states: ['sculk_shrieker[shrieking=true]', 'sculk_shrieker[shrieking=false]'] },
    fire_packets: { packets: [[-9, Y + 1, 5]] },
    door_packets: { packets: [[-4, Y, -10], [-4, Y + 1, -10]] },
    door_items: `/execute if entity @e[type=item,x=-4,y=${Y},z=-10,distance=..4]`,
    sapling_packets: { packets: [[6, Y, -10]] }
  },
  ticks: 40
})
