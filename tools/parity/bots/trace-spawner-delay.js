// Tick trace: a spawner placed with `Delay:-1` picks its delay and counts it down in the same
// tick (vanilla `BaseSpawner.serverTick`), so it spawns on tick MinSpawnDelay + 1 and every
// MinSpawnDelay + 1 ticks after. Armor stands have no spawn rules, so only free space matters,
// and six tries over a wide range all but rule out a tick where every try hits the spawner.
const { trace, PLATFORM_Y } = require('./steptrace')
const Y = PLATFORM_Y + 5

trace({
  name: 'spawner-delay',
  setup: [
    `/setblock 4 ${Y} 4 spawner{Delay:-1s,MinSpawnDelay:10s,MaxSpawnDelay:10s,SpawnCount:6,SpawnRange:4,MaxNearbyEntities:100,SpawnData:{entity:{id:"minecraft:armor_stand"}}}`
  ],
  probes: {
    delay: `/data get block 4 ${Y} 4 Delay`,
    spawned: '/execute if entity @e[type=armor_stand,limit=1]'
  },
  ticks: 24
})
