// Tick trace: breaking a stone block drops an item with random offset, yaw and motion. Needs the
// RNG lockdown (`--rng-seed`) for the two servers to draw the same values.
const { trace, PLATFORM_Y } = require('./steptrace')
const Y = PLATFORM_Y + 1
const item = '@e[type=item,distance=..20,sort=nearest,limit=1]'

trace({
  name: 'item-drop',
  setup: [`/setblock 3 ${Y} 3 stone`, `/setblock 3 ${Y} 3 air destroy`],
  probes: {
    pos: `/data get entity ${item} Pos`,
    motion: `/data get entity ${item} Motion`,
    rotation: `/data get entity ${item} Rotation`
  },
  ticks: 25
})
