// A fishing hook cast at an area effect cloud flies through it (clouds aren't pickable); cast
// at a ghast fireball it hooks it and sends it where the angler looks (`onRedirectProjectile`).
const { make } = require('./survival-lib')
const h = make('Angler')
const { bot } = h
const X = 3000; const Y = 200; const Z = 0
const HOOKED = 8

const named = (name) => Object.values(bot.entities).filter(e => e.name === name)
const velocities = {}
bot._client.on('entity_velocity', (p) => {
  const v = p.velocity
  ;(velocities[p.entityId] = velocities[p.entityId] || []).push(`${v.x.toFixed(2)},${v.y.toFixed(2)},${v.z.toFixed(2)}`)
})

async function cast (type, nbt) {
  h.cmd('kill @e[type=!player]')
  await h.ticks(10)
  h.cmd(`summon ${type} ${X + 0.5} ${Y + 1.4} ${Z + 3.5} ${nbt}`)
  let target
  await h.waitFor(() => (target = named(type)[0]), 5000)
  await bot.lookAt(new h.Vec3(X + 0.5, Y + 1.62, Z + 3.5), true)
  await h.ticks(5)
  bot.activateItem()
  await h.ticks(20)
  const bobber = named('fishing_bobber')[0]
  const hooked = bobber && bobber.metadata ? bobber.metadata[HOOKED] : undefined
  h.result(`${type}.hooked`, hooked === undefined ? (bobber ? 'none' : 'no_bobber') : hooked === target.id + 1)
  h.result(`${type}.velocities`, (velocities[target.id] || ['none']).join(';'))
  bot.activateItem()
  await h.ticks(5)
}

h.start(async () => {
  await h.arena(X, Y, Z, 6)
  h.cmd(`gamemode creative ${bot.username}`)
  await h.give('fishing_rod')
  await h.ticks(5)
  await cast('area_effect_cloud', '{Radius:0.5f,Duration:600,WaitTime:0}')
  await cast('fireball', '{Motion:[0d,0d,0d],acceleration_power:0d}')
  h.cmd('kill @e[type=!player]')
})
