// Tick trace: a boat slides along blue ice into a resting boat and pushes it.
const { trace, PLATFORM_Y } = require('./steptrace')
const Y = PLATFORM_Y

trace({
  name: 'boat-push',
  setup: [
    `/fill -6 ${Y} -4 6 ${Y} -4 blue_ice`,
    `/summon oak_boat 3.5 ${Y + 1} -3.5`,
    `/summon oak_boat 0.5 ${Y + 1} -3.5 {Motion:[1.0d,0.0d,0.0d],Tags:["mover"]}`
  ],
  probes: {
    mover: '/data get entity @e[type=oak_boat,tag=mover,limit=1] Pos',
    mover_motion: '/data get entity @e[type=oak_boat,tag=mover,limit=1] Motion',
    other: '/data get entity @e[type=oak_boat,tag=!mover,limit=1] Pos',
    other_motion: '/data get entity @e[type=oak_boat,tag=!mover,limit=1] Motion'
  },
  ticks: 30
})
