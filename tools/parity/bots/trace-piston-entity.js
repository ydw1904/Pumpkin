// Tick trace: a piston pushes the stone block an armor stand stands on. The moving block keeps
// colliding with the stand (`MovingPistonBlock.getCollisionShape`), so the stand only falls
// once the block has moved out from under it.
const { trace, PLATFORM_Y } = require('./steptrace')
const Y = PLATFORM_Y + 1

trace({
  name: 'piston-entity',
  setup: [
    `/setblock 5 ${Y} 0 piston`,
    `/setblock 5 ${Y} -1 stone`,
    `/summon armor_stand 5.5 ${Y + 1} -0.5`,
    `/setblock 5 ${Y} 1 redstone_block`
  ],
  probes: {
    stand: '/data get entity @e[type=armor_stand,limit=1] Pos',
    server_front: { server: [5, Y, -1], states: ['stone', 'moving_piston', 'piston_head', 'air'] }
  },
  ticks: 8
})
