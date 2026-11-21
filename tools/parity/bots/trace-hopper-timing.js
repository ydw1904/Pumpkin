// Tick trace: exact hopper transfer ticks. A hopper moves an item every 8 ticks (vanilla
// `HopperBlockEntity.pushItemsTick` decrements the cooldown before testing it), a hopper that
// just received its first item into an empty inventory waits 8 ticks, or 7 when it already ticked
// this game tick (`tryMoveInItem` compares `tickedGameTime`), and an item entity landing in a
// hopper is pulled in during the entity tick (`HopperBlock.entityInside`).
const { trace, PLATFORM_Y } = require('./steptrace')
const Y = PLATFORM_Y + 1
const count = (x, y, z, item) => `/execute if items block ${x} ${y} ${z} container.* ${item}`
const cooldown = (x, y, z) => `/data get block ${x} ${y} ${z} TransferCooldown`

trace({
  name: 'hopper-timing',
  setup: [
    // Chest above two hoppers above a chest.
    `/setblock 2 ${Y} 2 chest`,
    `/setblock 2 ${Y + 1} 2 hopper[facing=down]`,
    `/setblock 2 ${Y + 2} 2 hopper[facing=down]`,
    `/setblock 2 ${Y + 3} 2 chest`,
    `/item replace block 2 ${Y + 3} 2 container.0 with cobblestone 64`,
    // A filled hopper pushing sideways into an empty hopper, which pushes into a chest. Block
    // entities tick in the order they were added, so the sender ticks first.
    `/setblock 7 ${Y} 2 chest`,
    `/setblock 5 ${Y} 2 hopper[facing=east]`,
    `/setblock 6 ${Y} 2 hopper[facing=east]`,
    `/item replace block 5 ${Y} 2 container.0 with dirt 3`,
    // The same with the receiving hopper added, and ticking, before the sender.
    `/setblock -7 ${Y} 2 chest`,
    `/setblock -6 ${Y} 2 hopper[facing=west]`,
    `/setblock -5 ${Y} 2 hopper[facing=west]`,
    `/item replace block -5 ${Y} 2 container.0 with sand 3`,
    // An item entity resting in a hopper's bowl, over a chest.
    `/setblock 2 ${Y} -4 chest`,
    `/setblock 2 ${Y + 1} -4 hopper[facing=down]`,
    `/summon item 2.5 ${Y + 1.7} -3.5 {Item:{id:"minecraft:stone",count:3},PickupDelay:32767s,NoGravity:1b}`
  ],
  probes: {
    vertical_top: count(2, Y + 2, 2, 'cobblestone'),
    vertical_low: count(2, Y + 1, 2, 'cobblestone'),
    vertical_low_cooldown: cooldown(2, Y + 1, 2),
    vertical_chest: count(2, Y, 2, 'cobblestone'),
    east_first: count(5, Y, 2, 'dirt'),
    east_second: count(6, Y, 2, 'dirt'),
    east_second_cooldown: cooldown(6, Y, 2),
    east_chest: count(7, Y, 2, 'dirt'),
    west_second: count(-6, Y, 2, 'sand'),
    west_second_cooldown: cooldown(-6, Y, 2),
    west_chest: count(-7, Y, 2, 'sand'),
    entity_hopper: count(2, Y + 1, -4, 'stone'),
    entity_hopper_cooldown: cooldown(2, Y + 1, -4),
    entity_chest: count(2, Y, -4, 'stone')
  },
  ticks: 40
})
