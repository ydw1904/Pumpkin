// Raiders outside the raid itself: raiders only go for ominous banners, patrol captains always
// drop their banner and an ominous bottle, patrol members follow their captain, and a rung bell
// swings and makes raiders within 48 blocks glow after its resonance.
const { make } = require('./survival-lib')
const h = make('Patroller', 900000)
const { bot, Vec3 } = h
const X = 7400; const Y = 200; const Z = 0

const near = (e, x, z, r) => Math.hypot(e.position.x - x, e.position.z - z) < r
const mobs = (name, x, z, r = 40) => Object.values(bot.entities).filter(e => e.name === name && near(e, x, z, r))
const items = (x, z, r = 40) => Object.values(bot.entities).filter(e => e.name === 'item' && near(e, x, z, r) && e.getDroppedItem())
const itemNames = (x, z, r) => items(x, z, r).map(e => e.getDroppedItem().name)
const head = (e) => { const it = (e.equipment || [])[5]; return it ? it.name : 'none' }
const glowing = (e) => Boolean(e && typeof e.metadata[0] === 'number' && (e.metadata[0] & 0x40))
const bellSwings = []
bot._client.on('block_action', (p) => { if (p.location && p.location.x === X && p.location.z === Z + 40) bellSwings.push(`${p.byte1 ?? p.actionId}:${p.byte2 ?? p.actionParam}`) })

h.start(async () => {
  h.cmd('difficulty normal')
  h.cmd('time set 6000')
  h.cmd('weather clear')
  await h.arena(X, Y, Z, 20)
  h.cmd(`gamemode creative ${bot.username}`)
  await h.ticks(10)

  // A raider without a raid walks past a dropped stone and leaves it alone; a raider only takes
  // ominous banners, and only for a raid without a captain.
  h.cmd(`summon vindicator ${X + 4} ${Y + 1} ${Z} {PersistenceRequired:1b}`)
  h.cmd(`summon item ${X + 8} ${Y + 1} ${Z} {Item:{id:"minecraft:stone",count:1},PickupDelay:32767s}`)
  await h.waitFor(() => mobs('vindicator', X, Z).length === 1 && itemNames(X, Z).includes('stone'), 5000)
  await h.ticks(200)
  h.result('stone_left_alone', itemNames(X, Z).includes('stone'))
  h.result('vindicator_head_after_stone', head(mobs('vindicator', X, Z)[0] || {}))
  h.cmd(`kill @e[type=!player,x=${X},y=${Y},z=${Z},distance=..60]`)
  await h.waitFor(() => mobs('vindicator', X, Z).length === 0 && items(X, Z).length === 0, 5000)

  // Captains: pillagers summoned without data become captains 6% of the time; each captain
  // always drops its banner and, killed by a player outside a raid, an ominous bottle.
  h.cmd(`tp ${bot.username} ${X + 0.5} ${Y + 5} ${Z + 0.5}`)
  for (let dx = -14; dx <= 14; dx += 2) for (let dz = -9; dz <= 9; dz += 2) h.cmd(`summon pillager ${X + dx + 0.5} ${Y + 1} ${Z + dz + 0.5}`)
  await h.waitFor(() => mobs('pillager', X, Z, 30).length >= 150, 20000)
  await h.ticks(20)
  const captains = mobs('pillager', X, Z, 30).filter(e => head(e) === 'white_banner').length
  console.log(`captains ${captains}`)
  h.result('captains_spawned', captains > 0)
  h.cmd(`execute as @e[type=pillager,x=${X},y=${Y},z=${Z},distance=..30] run damage @s 1000 minecraft:player_attack by ${bot.username}`)
  await h.waitFor(() => mobs('pillager', X, Z, 30).length === 0, 10000)
  await h.ticks(40)
  const drops = items(X, Z, 30).flatMap(e => Array(e.getDroppedItem().count).fill(e.getDroppedItem().name))
  const banners = drops.filter(n => n === 'white_banner').length
  const bottles = drops.filter(n => n === 'ominous_bottle').length
  console.log(`banners ${banners} bottles ${bottles}`)
  h.result('every_captain_drops_banner', banners === captains)
  h.result('every_captain_drops_bottle', bottles === captains)
  h.cmd(`kill @e[type=!player,x=${X},y=${Y},z=${Z},distance=..60]`)
  await h.waitFor(() => items(X, Z).length === 0, 5000)

  // A patrol: the captain heads for its target ten blocks at a time and hands each leg to the
  // members within 16 blocks, so the group moves together.
  const PX = X - 15; const PZ = Z - 15
  h.cmd(`tp ${bot.username} ${X + 0.5} ${Y + 12} ${Z + 0.5}`)
  h.cmd(`summon pillager ${PX + 0.5} ${Y + 1} ${PZ + 0.5} {PersistenceRequired:1b,PatrolLeader:1b,Patrolling:1b,patrol_target:[I;${X + 15},${Y + 1},${Z + 15}],Tags:["captain"]}`)
  h.cmd(`summon pillager ${PX + 2.5} ${Y + 1} ${PZ + 0.5} {PersistenceRequired:1b,Patrolling:1b}`)
  h.cmd(`summon pillager ${PX + 0.5} ${Y + 1} ${PZ + 2.5} {PersistenceRequired:1b,Patrolling:1b}`)
  await h.waitFor(() => mobs('pillager', PX, PZ, 6).length === 3, 5000)
  const start = new Map(mobs('pillager', PX, PZ, 6).map(e => [e.id, e.position.clone()]))
  for (let i = 0; i < 8; i++) {
    await h.serverTicks(40)
    console.log(`patrol t${(i + 1) * 40} ${[...start.keys()].map(id => { const e = bot.entities[id]; return e ? `${head(e)}@${Math.round(e.position.x - X)},${Math.round(e.position.z - Z)}` : 'gone' }).join(' ')}`)
  }
  const moved = [...start].map(([id, p]) => { const e = bot.entities[id]; return e ? Math.round(e.position.distanceTo(p)) : -1 })
  console.log(`patrol moved ${moved.join(',')}`)
  h.result('patrol_all_moved', moved.every(d => d >= 6))
  const now = [...start.keys()].map(id => bot.entities[id]).filter(Boolean)
  const spread = Math.max(...now.flatMap(a => now.map(b => a.position.distanceTo(b.position))))
  h.result('patrol_kept_together', spread < 12)
  h.cmd(`kill @e[type=pillager,x=${X},y=${Y},z=${Z},distance=..60]`)
  await h.waitFor(() => mobs('pillager', X, Z, 60).length === 0, 5000)

  // A bell: the ring is a block event that swings the bell for everyone; 45 ticks later the
  // raiders within 48 blocks of it glow for three seconds.
  h.cmd(`fill ${X - 62} ${Y} ${Z + 36} ${X + 32} ${Y} ${Z + 44} stone`)
  h.cmd(`setblock ${X} ${Y + 1} ${Z + 40} bell[attachment=floor,facing=north]`)
  h.cmd(`summon vindicator ${X + 30} ${Y + 1} ${Z + 40} {NoAI:1b,PersistenceRequired:1b}`)
  h.cmd(`summon vindicator ${X - 60} ${Y + 1} ${Z + 40} {NoAI:1b,PersistenceRequired:1b}`)
  await h.tp(X + 0.5, Y + 1, Z + 38.5)
  await h.waitFor(() => mobs('vindicator', X, Z + 40, 70).length === 2, 5000)
  await h.ticks(20)
  const close = mobs('vindicator', X + 30, Z + 40, 5)[0]
  const far = mobs('vindicator', X - 60, Z + 40, 5)[0]
  bot.lookAt(new Vec3(X + 0.5, Y + 1.5, Z + 40.5), true)
  await h.ticks(2)
  await bot.activateBlock(h.block(X, Y + 1, Z + 40), new Vec3(0, 0, -1), new Vec3(0.5, 0.5, 0))
  await h.waitFor(() => glowing(close), 5000)
  h.result('bell_swing_events', bellSwings.join(',') || 'none')
  h.result('bell_near_raider_glows', glowing(close))
  h.result('bell_far_raider_glows', glowing(far))
  await h.waitFor(() => !glowing(close), 8000)
  h.result('bell_glow_ends', !glowing(close))
  h.cmd(`kill @e[type=vindicator,x=${X},y=${Y},z=${Z},distance=..100]`)
  await h.ticks(10)
})
