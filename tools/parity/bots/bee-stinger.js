// The bot punches a bee, which stings back: vanilla `Bee.doHurtTarget` adds a stinger stuck in the
// bot (`DATA_STINGER_COUNT_ID`, metadata 13), and `LivingEntity.tick` drops it after 580 ticks.
const { make } = require('./survival-lib')
const h = make('Stung')
const { bot } = h
const X = 9000; const Y = 200; const Z = 0

let stingers = 0
bot._client.on('entity_metadata', (p) => {
  if (p.entityId !== bot.entity.id) return
  for (const m of p.metadata) if (m.key === 13) stingers = m.value
})

h.start(async () => {
  h.cmd('difficulty easy')
  await h.arena(X, Y, Z, 6)
  h.cmd(`summon bee ${X + 2.5} ${Y + 1} ${Z + 0.5} {PersistenceRequired:1b}`)
  await h.waitFor(() => Object.values(bot.entities).some((e) => e.name === 'bee'), 5000)
  await h.ticks(10)
  const bee = Object.values(bot.entities).find((e) => e.name === 'bee')
  const before = bot.health
  if (bee) bot.attack(bee)
  const stung = await h.waitFor(() => bot.health < before - 0.5, 20000)
  await h.ticks(5)
  h.result('stung', stung)
  h.result('stingers_after_sting', stingers)
  h.cmd('kill @e[type=bee]')
  await h.serverTicks(600)
  h.result('stingers_after_600_ticks', stingers)
})
