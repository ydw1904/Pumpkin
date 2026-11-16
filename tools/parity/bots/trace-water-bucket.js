// Tick trace: a water source spreads for 20 ticks, a dispenser takes it with a bucket, and the
// flowing water around it refills the cell and then drains. The order fluid ticks run in decides
// the levels on the way.
const { trace, PLATFORM_Y } = require('./steptrace')
const Y = PLATFORM_Y + 1
trace({
  name: 'water-bucket',
  setup: [
    `/fill 0 ${Y} 2 2 ${Y} 2 stone`,
    `/fill 0 ${Y} 4 2 ${Y} 4 stone`,
    `/setblock 2 ${Y} 3 dispenser[facing=east]`,
    `/item replace block 2 ${Y} 3 container.0 with bucket 2`,
    `/setblock 1 ${Y} 3 repeater[facing=west,delay=4]`,
    `/setblock 0 ${Y} 3 repeater[facing=west,delay=4]`,
    `/setblock 3 ${Y} 3 water`,
    `/setblock -1 ${Y} 3 redstone_block`
  ],
  probes: {
    c: { block: [3, Y, 3] },
    e1: { block: [4, Y, 3] },
    e2: { block: [5, Y, 3] },
    e3: { block: [6, Y, 3] },
    n1: { block: [3, Y, 2] },
    ne: { block: [4, Y, 2] },
    items: '/data get block 2 ' + Y + ' 3 Items'
  },
  ticks: 60
})
