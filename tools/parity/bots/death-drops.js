// Dying drops the main inventory, armor and offhand (not curse of vanishing items) and
// min(level * 7, 100) experience; a respawned player can walk back and collect both.
const { make } = require('./survival-lib')
const h = make('Dier')
const { bot } = h
const X = 1400; const Y = 200; const Z = 0

h.start(async () => {
  await h.arena(X, Y, Z, 12)
  await h.clear()
  h.cmd(`item replace entity ${bot.username} armor.head with iron_helmet`)
  h.cmd(`item replace entity ${bot.username} armor.chest with iron_chestplate`)
  h.cmd(`item replace entity ${bot.username} armor.legs with leather_leggings[enchantments={vanishing_curse:1}]`)
  h.cmd(`item replace entity ${bot.username} armor.feet with golden_boots`)
  h.cmd(`item replace entity ${bot.username} weapon.offhand with shield`)
  await h.give('cobblestone', 64)
  await h.give('dirt', 10)
  await h.give('stone_sword[enchantments={vanishing_curse:1}]', 1)
  h.cmd(`xp set ${bot.username} 30 levels`)
  await h.waitFor(() => bot.experience.level === 30, 3000)
  await h.ticks(10)
  // `/item replace` must not leave copies in other client slots.
  h.result('slots_before', h.slots())
  const deathPos = bot.entity.position.clone()
  h.cmd(`kill ${bot.username}`)
  await h.respawnAfterDeath()
  h.result('respawn_slots', h.slots())
  h.result('respawn_level', bot.experience.level)
  await h.tp(deathPos.x, deathPos.y, deathPos.z)
  // Items have a 40 tick pickup delay and orbs fly to the player; collect what is in reach.
  await h.ticks(100)
  const near = Object.values(bot.entities).filter(e => e.name === 'item' && e.position.distanceTo(deathPos) < 16 && e.getDroppedItem())
  const names = new Set(near.map(e => e.getDroppedItem().name).concat(Object.keys(h.inv())))
  h.result('dropped_items', [...names].sort().join(','))
  h.result('xp_level_recovered', bot.experience.level)

  // Mob equipment drops only on a player kill, unless the item was picked up (chance above 1).
  await h.clear()
  const zx = X + 3.5; const zz = Z + 0.5
  const zombie = (helmet, chance) => h.cmd(`summon zombie ${zx} ${Y + 1} ${zz} {NoAI:1b,Silent:1b,Health:0.1f,equipment:{head:{id:"minecraft:${helmet}",count:1}},drop_chances:{head:${chance}f}}`)
  zombie('diamond_helmet', 1.0)
  zombie('golden_helmet', 2.0)
  await h.ticks(10)
  h.cmd(`kill @e[type=zombie,x=${zx},y=${Y + 1},z=${zz},distance=..3]`)
  await h.ticks(10)
  // Killed zombies linger for their death animation; only the new one is the target.
  const dead = new Set(Object.values(bot.entities).filter(e => e.name === 'zombie').map(e => e.id))
  const fresh = () => Object.values(bot.entities).find(e => e.name === 'zombie' && !dead.has(e.id))
  zombie('chainmail_helmet', 1.0)
  await h.tp(X + 1.5, Y + 1, zz, -90, 0)
  await h.waitFor(() => fresh(), 3000)
  const target = fresh()
  // Armor softens the punch, so the zombie has next to no health.
  await h.ticks(30)
  if (target) {
    await bot.lookAt(target.position.offset(0, 1, 0), true)
    bot.attack(target)
  }
  await h.ticks(40)
  const helmets = ['diamond_helmet', 'golden_helmet', 'chainmail_helmet']
  const seen = Object.values(bot.entities).filter(e => e.name === 'item' && e.getDroppedItem()).map(e => e.getDroppedItem().name).concat(Object.keys(h.inv()))
  h.result('mob_equipment_drops', helmets.filter(n => seen.includes(n)).join(',') || 'none')

  // A natural creaking heart blown up by the bot's TNT kills its creaking with the death effects
  // (it lingers for the death animation).
  await h.clear()
  h.cmd(`effect give ${bot.username} resistance infinite 255 true`)
  const hx = X - 6; const hz = Z - 6
  h.cmd(`setblock ${hx} ${Y + 1} ${hz} pale_oak_log[axis=y]`)
  h.cmd(`setblock ${hx} ${Y + 3} ${hz} pale_oak_log[axis=y]`)
  h.cmd(`setblock ${hx} ${Y + 2} ${hz} creaking_heart[axis=y,natural=true,creaking_heart_state=awake]{creaking:[I;1,2,3,4]}`)
  h.cmd(`summon creaking ${hx + 3.5} ${Y + 1} ${hz + 0.5} {NoAI:1b,PersistenceRequired:1b,UUID:[I;1,2,3,4],home_pos:[I;${hx},${Y + 2},${hz}]}`)
  let creaking
  await h.waitFor(() => (creaking = Object.values(bot.entities).find(e => e.name === 'creaking')), 3000)
  let died = false
  bot._client.on('entity_status', (p) => { if (creaking && p.entityId === creaking.id && p.entityStatus === 3) died = true })
  await h.tp(hx + 2.5, Y + 1, hz + 3.5)
  await h.serverTicks(5)
  const owner = bot.player.uuid.replace(/-/g, '').match(/.{8}/g).map(part => parseInt(part, 16) | 0).join(',')
  for (const [dx, dz] of [[1, 0], [-1, 0], [0, 1], [0, -1]]) {
    h.cmd(`summon tnt ${hx + dx + 0.5} ${Y + 2} ${hz + dz + 0.5} {fuse:0,NoGravity:1b,owner:[I;${owner}]}`)
  }
  await h.serverTicks(10)
  const heart = h.blockName(hx, Y + 2, hz)
  const lingering = !!(creaking && bot.entities[creaking.id])
  h.result('creaking_heart_tnt', `heart=${heart} died=${died} lingering=${lingering}`)
})
