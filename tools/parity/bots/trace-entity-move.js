// Tick trace: entities moving through vanilla `Entity.move`. Armor stands jump forward on
// stone, soul sand and honey and slide on ice into a wall; an item, a falling block and TNT are
// thrown into walls; an XP orb (out of the bot's follow range) bounces off a wall; an armor
// stand and an item fall onto slime blocks.
const { trace, PLATFORM_Y } = require('./steptrace')
const Y = PLATFORM_Y
const stand = (tag) => `@e[type=armor_stand,tag=${tag},distance=..30,limit=1]`
const jump = 'Motion:[0.3d,0.42d,0.0d]'

trace({
  name: 'entity-move',
  setup: [
    `/fill -6 ${Y} -8 8 ${Y} -8 soul_sand`,
    `/fill -6 ${Y} -6 8 ${Y} -6 honey_block`,
    `/fill -6 ${Y} -4 8 ${Y} -4 ice`,
    `/fill 2 ${Y + 1} -4 2 ${Y + 2} -4 stone`,
    `/fill 4 ${Y + 1} 3 4 ${Y + 4} 9 stone`,
    `/fill -11 ${Y + 1} 10 -11 ${Y + 2} 10 stone`,
    `/setblock -3 ${Y} -12 slime_block`,
    `/setblock 6 ${Y} -12 slime_block`,
    `/summon armor_stand -2.5 ${Y + 4} -11.5 {Tags:["slime"]}`,
    `/summon item 6.5 ${Y + 4} -11.5 {Item:{id:"minecraft:dirt",count:1},PickupDelay:32767}`,
    `/summon armor_stand -3.5 ${Y + 1} -10.5 {Tags:["stone"],${jump}}`,
    `/summon armor_stand -3.5 ${Y + 0.875} -7.5 {Tags:["soul"],${jump}}`,
    `/summon armor_stand -3.5 ${Y + 0.9375} -5.5 {Tags:["honey"],${jump}}`,
    `/summon armor_stand -3.5 ${Y + 1} -3.5 {Tags:["ice"],Motion:[0.8d,0.0d,0.0d]}`,
    `/summon item 0.5 ${Y + 1} 3.5 {Item:{id:"minecraft:stone",count:1},PickupDelay:32767,Motion:[0.6d,0.2d,0.0d]}`,
    `/summon falling_block 0.5 ${Y + 3} 7.5 {BlockState:{Name:"minecraft:sand"},Motion:[0.4d,0.0d,0.0d]}`,
    `/summon tnt 0.5 ${Y + 1} 9.5 {fuse:100,Motion:[0.8d,0.3d,0.0d]}`,
    `/summon experience_orb -8.5 ${Y + 1} 10.5 {Value:1,Motion:[-0.5d,0.3d,0.0d]}`
  ],
  probes: {
    stone: `/data get entity ${stand('stone')} Pos`,
    stone_motion: `/data get entity ${stand('stone')} Motion`,
    soul: `/data get entity ${stand('soul')} Pos`,
    soul_motion: `/data get entity ${stand('soul')} Motion`,
    honey: `/data get entity ${stand('honey')} Pos`,
    honey_motion: `/data get entity ${stand('honey')} Motion`,
    ice: `/data get entity ${stand('ice')} Pos`,
    ice_motion: `/data get entity ${stand('ice')} Motion`,
    item: '/data get entity @e[type=item,distance=..30,sort=nearest,limit=1] Pos',
    item_motion: '/data get entity @e[type=item,distance=..30,sort=nearest,limit=1] Motion',
    sand: '/data get entity @e[type=falling_block,distance=..30,sort=nearest,limit=1] Pos',
    sand_motion: '/data get entity @e[type=falling_block,distance=..30,sort=nearest,limit=1] Motion',
    tnt: '/data get entity @e[type=tnt,distance=..30,sort=nearest,limit=1] Pos',
    tnt_motion: '/data get entity @e[type=tnt,distance=..30,sort=nearest,limit=1] Motion',
    orb: '/data get entity @e[type=experience_orb,distance=..30,sort=nearest,limit=1] Pos',
    slime: `/data get entity ${stand('slime')} Pos`,
    slime_motion: `/data get entity ${stand('slime')} Motion`,
    slime_item: `/data get entity @e[type=item,x=6.5,y=${Y},z=-11.5,distance=..8,sort=nearest,limit=1] Pos`,
    slime_item_motion: `/data get entity @e[type=item,x=6.5,y=${Y},z=-11.5,distance=..8,sort=nearest,limit=1] Motion`,
    orb_motion: '/data get entity @e[type=experience_orb,distance=..30,sort=nearest,limit=1] Motion'
  },
  ticks: 30
})
