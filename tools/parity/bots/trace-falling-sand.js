// Tick trace: a sand block falls 9 blocks onto the platform.
const { trace, PLATFORM_Y } = require('./steptrace')
const Y = PLATFORM_Y

trace({
  name: 'falling-sand',
  setup: [`/setblock 3 ${Y + 10} 3 sand`],
  probes: {
    pos: '/data get entity @e[type=falling_block,distance=..20,sort=nearest,limit=1] Pos',
    motion: '/data get entity @e[type=falling_block,distance=..20,sort=nearest,limit=1] Motion',
    source: { block: [3, Y + 10, 3] },
    landed: { block: [3, Y + 1, 3] }
  },
  ticks: 20
})
