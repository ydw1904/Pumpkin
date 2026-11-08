// Projectiles a survival player punches (a ghast fireball sent back kills the ghast, a wind
// charge flies where the player looks, a blaze fireball can't be punched), arrows and tridents
// hitting mobs early and late in their flight (the entity hit margin grows over the first ticks),
// from inside a mob and through two mobs, where they come to rest, a fishing hook hooking a mob,
// and the fireball `Item` tag.
const { make } = require('./survival-lib')
const h = make('Deflector')
const { bot } = h
const X = 9800; const Y = 200; const Z = -800

const r2 = (n) => Math.round(n * 100) / 100
const r3 = (n) => Math.round(n * 1000) / 1000
const HEALTH = 9
const velocities = {}
const explosions = []
bot._client.on('entity_velocity', (p) => {
  if (bot.entity && p.entityId === bot.entity.id) return
  const v = p.velocity
  ;(velocities[p.entityId] ||= []).push([r3(v.x), r3(v.y), r3(v.z)])
})
bot._client.on('explosion', (p) => {
  const c = p.center || p
  explosions.push([r2(c.x), r2(c.y), r2(c.z)])
})

// Pumpkin sends a projectile's motion every tick, vanilla only when it changes, so only the
// moving ones after `from` count.
const moving = (e, from, n = 1) => (velocities[e.id] || []).slice(from)
  .filter(v => v.some(c => c !== 0)).slice(0, n).map(v => v.join(',')).join(' ') || 'none'
const near = (name) => Object.values(bot.entities)
  .filter(e => e.name === name && e.position.distanceTo(bot.entity.position) < 30)
  .sort((a, b) => a.position.distanceTo(bot.entity.position) - b.position.distanceTo(bot.entity.position))[0]
const health = (e) => (e && e.metadata && typeof e.metadata[HEALTH] === 'number') ? r2(e.metadata[HEALTH]) : 'gone'
const pos = (e) => e ? `${r2(e.position.x - X)},${r2(e.position.y - Y)},${r2(e.position.z - Z)}` : 'gone'
const itemName = (e) => {
  const m = e && e.metadata && e.metadata[8]
  if (!m) return 'none'
  const id = m.itemId ?? m.id ?? m.blockId
  const item = id !== undefined && bot.registry.items[id]
  return item ? item.name : JSON.stringify(m).slice(0, 60)
}

async function reset () {
  h.cmd(`kill @e[type=!player,distance=..40,x=${X},y=${Y},z=${Z}]`)
  h.cmd(`effect give ${bot.username} instant_health 1 5`)
  h.cmd(`fill ${X - 4} ${Y + 1} ${Z + 11} ${X + 4} ${Y + 6} ${Z + 11} stone`)
  await h.tp(X + 0.5, Y + 1, Z + 0.5, 0, 0)
  // Killed mobs stay for their 20 tick death animation.
  await h.serverTicks(40)
}

async function spawned (name) {
  await h.waitFor(() => near(name), 5000)
  return near(name)
}

h.start(async () => {
  await h.arena(X, Y, Z, 12)
  h.cmd('difficulty normal')
  h.cmd('gamerule fall_damage false')
  h.cmd('gamerule mob_griefing false')

  // A ghast fireball punched back goes where the player looks, belongs to the player and kills
  // the ghast it hits.
  await reset()
  h.cmd(`summon ghast ${X + 0.5} ${Y + 1} ${Z + 7.5} {NoAI:1b}`)
  h.cmd(`summon fireball ${X + 0.5} ${Y + 2.1} ${Z + 2.5} {Motion:[0d,0d,0d],acceleration_power:0d}`)
  let fireball = await spawned('fireball')
  const ghast = await spawned('ghast')
  await h.serverTicks(10)
  let nExp = explosions.length
  let nVel = (velocities[fireball.id] || []).length
  bot.attack(fireball)
  await h.waitFor(() => explosions.length > nExp, 5000)
  await h.serverTicks(40)
  h.result('ghast_fireball.first_velocity', moving(fireball, nVel))
  h.result('ghast_fireball.explosion', explosions.length > nExp ? explosions[nExp].map((v, i) => r2(v - [X, Y, Z][i])).join(',') : 'none')
  h.result('ghast_fireball.ghast', health(bot.entities[ghast.id]))

  // Blaze fireballs aren't redirectable: the punch does nothing to them.
  await reset()
  h.cmd(`summon small_fireball ${X + 0.5} ${Y + 2.1} ${Z + 2.5} {Motion:[0d,0d,0d],acceleration_power:0d}`)
  const small = await spawned('small_fireball')
  await h.serverTicks(10)
  bot.attack(small)
  await h.serverTicks(10)
  h.result('small_fireball.moving_velocities', (velocities[small.id] || []).filter(v => v.some(c => c !== 0)).length)
  h.result('small_fireball.pos', pos(bot.entities[small.id]))

  // A wind charge past its first five ticks can be punched; it explodes against the wall.
  await reset()
  h.cmd(`summon wind_charge ${X + 0.5} ${Y + 2.1} ${Z + 2.5} {Motion:[0d,0d,0d],acceleration_power:0d}`)
  const charge = await spawned('wind_charge')
  await h.serverTicks(10)
  nExp = explosions.length
  nVel = (velocities[charge.id] || []).length
  bot.attack(charge)
  await h.waitFor(() => explosions.length > nExp, 5000)
  h.result('wind_charge.first_velocity', moving(charge, nVel))
  h.result('wind_charge.explosion', explosions.length > nExp ? explosions[nExp].map((v, i) => r2(v - [X, Y, Z][i])).join(',') : 'none')

  // A plain summoned wind charge gets the default acceleration power, which a later Motion uses:
  // it reaches the wall in about 6 ticks instead of about 21.
  await reset()
  h.cmd(`summon wind_charge ${X + 0.5} ${Y + 2.1} ${Z + 6.5}`)
  await spawned('wind_charge')
  await h.serverTicks(5)
  nExp = explosions.length
  h.cmd(`data merge entity @e[type=wind_charge,limit=1,sort=nearest,x=${X},y=${Y},z=${Z}] {Motion:[0d,0d,0.2d]}`)
  let waited = 0
  while (explosions.length === nExp && waited < 60) { await h.ticks(1); waited++ }
  h.result('plain_wind_charge.speed', explosions.length === nExp ? 'none' : waited < 14 ? 'accelerating' : 'constant')

  // Arrows pass 0.15 beside a husk: early in the flight they miss, later they hit.
  for (const [label, start, speed] of [['early', 4.5, 1.0], ['late', 2.5, 0.5]]) {
    await reset()
    h.cmd(`summon husk ${X + 0.5} ${Y + 1} ${Z + 6.5} {NoAI:1b,Silent:1b}`)
    const husk = await spawned('husk')
    h.cmd(`summon arrow ${X + 0.95} ${Y + 2} ${Z + start} {Motion:[0d,0d,${speed}d],NoGravity:1b,pickup:0b}`)
    await h.serverTicks(30)
    h.result(`arrow_graze.${label}`, health(bot.entities[husk.id]))
  }

  // An arrow that starts inside a husk doesn't hit it.
  await reset()
  h.cmd(`summon husk ${X + 0.5} ${Y + 1} ${Z + 6.5} {NoAI:1b,Silent:1b}`)
  let husk = await spawned('husk')
  h.cmd(`summon arrow ${X + 0.5} ${Y + 2} ${Z + 6.5} {Motion:[0d,0d,0.5d],NoGravity:1b,pickup:0b}`)
  await h.serverTicks(30)
  h.result('arrow_inside.husk', health(bot.entities[husk.id]))

  // A piercing arrow goes through two husks and sticks in the wall.
  await reset()
  h.cmd(`summon husk ${X + 0.5} ${Y + 1} ${Z + 6.5} {NoAI:1b,Silent:1b}`)
  h.cmd(`summon husk ${X + 0.5} ${Y + 1} ${Z + 8.5} {NoAI:1b,Silent:1b}`)
  husk = await spawned('husk')
  const others = () => Object.values(bot.entities).filter(e => e.name === 'husk' && e.id !== husk.id)
  await h.waitFor(() => others().length > 0, 5000)
  const second = others()[0]
  h.cmd(`summon arrow ${X + 0.5} ${Y + 2} ${Z + 1.5} {Motion:[0d,0d,2.5d],NoGravity:1b,PierceLevel:1b,pickup:0b}`)
  await h.serverTicks(30)
  h.result('arrow_pierce.first', health(bot.entities[husk.id]))
  h.result('arrow_pierce.second', health(second && bot.entities[second.id]))
  h.result('arrow_pierce.rest', pos(near('arrow')))

  // An arrow in the wall rests 0.05 back from where it hit, only along the axes it moved on.
  await reset()
  h.cmd(`summon arrow ${X + 0.5} ${Y + 2} ${Z + 6.5} {Motion:[0d,0d,1.3d],NoGravity:1b,pickup:0b}`)
  await h.serverTicks(20)
  h.result('arrow_wall.rest', pos(near('arrow')))

  // A trident in the wall rests like an arrow.
  await reset()
  h.cmd(`summon trident ${X + 0.5} ${Y + 2} ${Z + 6.5} {Motion:[0d,0d,1.3d],NoGravity:1b,pickup:0b}`)
  await h.serverTicks(20)
  h.result('trident_wall.rest', pos(near('trident')))

  // A trident hits the husk where its path meets the husk's box, then drops from there.
  await reset()
  h.cmd(`summon husk ${X + 0.5} ${Y + 1} ${Z + 6.5} {NoAI:1b,Silent:1b}`)
  husk = await spawned('husk')
  h.cmd(`summon trident ${X + 0.5} ${Y + 2} ${Z + 1.5} {Motion:[0d,0d,1.7d],pickup:0b}`)
  await h.serverTicks(40)
  h.result('trident_mob.husk', health(bot.entities[husk.id]))
  h.result('trident_mob.rest', pos(near('trident')))

  // A fishing hook cast at a husk hooks it.
  await reset()
  h.cmd(`summon husk ${X + 0.5} ${Y + 1} ${Z + 4.5} {NoAI:1b,Silent:1b}`)
  husk = await spawned('husk')
  await h.fill({ 0: 'fishing_rod' })
  bot.setQuickBarSlot(0)
  await h.ticks(5)
  bot.activateItem()
  await h.serverTicks(20)
  const hook = near('fishing_bobber')
  const hooked = hook && hook.metadata && hook.metadata[8]
  h.result('fishing.hooked', hooked ? (hooked - 1 === husk.id ? 'husk' : `other:${hooked}`) : 'none')
  bot.activateItem()
  await h.serverTicks(5)

  // Fireballs keep the item they were given, also after `/data merge` rewrites them.
  await reset()
  for (const name of ['fireball', 'small_fireball']) {
    h.cmd(`summon ${name} ${X + 0.5} ${Y + 3} ${Z + 4.5} {Motion:[0d,0d,0d],acceleration_power:0d,Item:{id:"minecraft:snowball",count:3}}`)
    fireball = await spawned(name)
    await h.serverTicks(5)
    const summoned = itemName(bot.entities[fireball.id])
    h.cmd(`data merge entity @e[type=${name},limit=1,sort=nearest,x=${X},y=${Y},z=${Z}] {NoGravity:1b}`)
    await h.serverTicks(5)
    h.result(`${name}.item`, `${summoned}/${itemName(bot.entities[fireball.id])}`)
    h.cmd(`kill @e[type=${name}]`)
  }
  h.cmd(`kill @e[type=!player,distance=..40,x=${X},y=${Y},z=${Z}]`)
})
