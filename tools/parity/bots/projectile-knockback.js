// A survival player hit by wind charges and a small fireball: the first motion it is sent and
// the explosion knockback. Both depend on where the projectile hits, so they catch differences
// in the entity hit margin, the stored acceleration power and the hit point.
const { make } = require('./survival-lib')
const h = make('Knocked')
const { bot } = h
const X = 9700; const Y = 200; const Z = -700

const r3 = (n) => Math.round(n * 1000) / 1000
let clientTick = 0
const own = []
const explosions = []
bot.on('physicsTick', () => { clientTick++ })
bot._client.on('entity_velocity', (p) => {
  if (!bot.entity || p.entityId !== bot.entity.id) return
  const v = p.velocity
  own.push([clientTick, r3(v.x), r3(v.y), r3(v.z)])
})
bot._client.on('explosion', (p) => {
  const k = p.playerKnockback
  explosions.push([clientTick, k ? [r3(k.x), r3(k.y), r3(k.z)] : 'none'])
})

async function reset () {
  h.cmd(`kill @e[type=!player,distance=..40,x=${X},y=${Y},z=${Z}]`)
  h.cmd(`effect give ${bot.username} instant_health 1 5`)
  await h.tp(X + 0.5, Y + 1, Z + 0.5, 0, 0)
  // Server ticks, so the last scene's knockback has died down on a slow server too.
  await h.serverTicks(40)
}

// Runs `trigger` and records the first own motion and explosion knockback after it.
async function scene (label, trigger) {
  await reset()
  const t0 = clientTick
  const nOwn = own.length; const nExp = explosions.length
  await trigger()
  if (!await h.waitFor(() => own.length > nOwn, 10000)) {
    h.result(`${label}.first`, 'none')
    return
  }
  await h.ticks(10)
  const first = own[nOwn]
  const ex = explosions.slice(nExp)
  h.result(`${label}.first_h`, r3(Math.hypot(first[1], first[3])))
  h.result(`${label}.first_y`, first[2])
  h.result(`${label}.explode`, ex.length ? ex[0][1] : 'none')
  console.log(`INFO ${label} vels=${JSON.stringify(own.slice(nOwn, nOwn + 4).map(([t, ...v]) => `${t - t0}:${v}`))} explode=${JSON.stringify(ex.map(([t, k]) => `${t - t0}:${k}`))}`)
}

h.start(async () => {
  await h.arena(X, Y, Z, 12)
  h.cmd('difficulty normal')
  h.cmd('gamerule fall_damage false')
  h.cmd('gamerule mob_griefing false')

  // Summoned charges read the default acceleration power of 0.1; thrown ones have none.
  await scene('wind_charge', async () => {
    h.cmd(`summon wind_charge ${X + 0.5} ${Y + 1.5} ${Z + 3.5} {Motion:[0.0d,0.0d,-1.0d]}`)
  })
  await scene('wind_charge_noaccel', async () => {
    h.cmd(`summon wind_charge ${X + 0.5} ${Y + 1.5} ${Z + 3.5} {Motion:[0.0d,0.0d,-1.0d],acceleration_power:0d}`)
  })
  await scene('wind_charge_slow', async () => {
    h.cmd(`summon wind_charge ${X + 0.5} ${Y + 1.5} ${Z + 4.5} {Motion:[0.0d,0.0d,-0.4d],acceleration_power:0d}`)
  })
  await scene('wind_charge_diagonal', async () => {
    h.cmd(`summon wind_charge ${X + 2.5} ${Y + 2.5} ${Z + 2.5} {Motion:[-0.5d,-0.3d,-0.5d],acceleration_power:0d}`)
  })
  await scene('breeze_wind_charge', async () => {
    h.cmd(`summon breeze_wind_charge ${X + 0.5} ${Y + 1.5} ${Z + 3.5} {Motion:[0.0d,0.0d,-0.7d],acceleration_power:0d}`)
  })
  await scene('small_fireball', async () => {
    h.cmd(`summon small_fireball ${X + 0.5} ${Y + 1.5} ${Z + 3.5} {Motion:[0.0d,0.0d,-0.6d]}`)
  })
  h.cmd(`kill @e[type=!player,distance=..40,x=${X},y=${Y},z=${Z}]`)
})
