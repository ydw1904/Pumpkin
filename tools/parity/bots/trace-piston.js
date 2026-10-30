// Tick trace: a redstone block powers a piston that pushes a stone block.
// Plain `piston` faces north: Pumpkin's /setblock can't parse block states yet (ROADMAP #7).
const { trace, PLATFORM_Y } = require('./steptrace')
const Y = PLATFORM_Y + 1

trace({
  name: 'piston',
  setup: [
    `/setblock 5 ${Y} 0 piston`,
    `/setblock 5 ${Y} -1 stone`,
    `/setblock 5 ${Y} 1 redstone_block`
  ],
  probes: {
    piston: { block: [5, Y, 0] },
    front: { block: [5, Y, -1] },
    pushed: { block: [5, Y, -2] },
    server_piston: { server: [5, Y, 0], states: ['piston[extended=false]', 'piston[extended=true]'] },
    server_front: { server: [5, Y, -1], states: ['stone', 'moving_piston', 'piston_head', 'air'] },
    server_pushed: { server: [5, Y, -2], states: ['air', 'moving_piston', 'stone'] },
    moving: `/data get block 5 ${Y} -2`
  },
  ticks: 6
})
