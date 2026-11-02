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
})
