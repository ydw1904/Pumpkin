// Tick trace: a repeater powers a piston from its scheduled tick, and vanilla runs the piston's
// block event in that same tick (`runBlockEvents` after the scheduled ticks in
// `ServerLevel.tick`).
const { trace, PLATFORM_Y } = require('./steptrace')
const Y = PLATFORM_Y + 1

trace({
  name: 'repeater-piston',
  setup: [
    `/setblock 5 ${Y} 0 piston[facing=north]`,
    `/setblock 5 ${Y} -1 stone`,
    `/setblock 5 ${Y} 1 repeater[facing=south,delay=1]`,
    `/setblock 5 ${Y} 2 redstone_block`
  ],
  probes: {
    server_repeater: { server: [5, Y, 1], states: ['repeater[powered=false]', 'repeater[powered=true]'] },
    server_piston: { server: [5, Y, 0], states: ['piston[extended=false]', 'piston[extended=true]'] },
    server_front: { server: [5, Y, -1], states: ['stone', 'moving_piston', 'piston_head', 'air'] },
    server_pushed: { server: [5, Y, -2], states: ['air', 'moving_piston', 'stone'] }
  },
  ticks: 7
})
