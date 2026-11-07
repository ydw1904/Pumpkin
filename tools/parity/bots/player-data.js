// How often a player's own entity data and attributes reach it: standing still, sneaking,
// taking damage, getting an effect. Vanilla sends dirty data in the tick it changes.
const { make } = require('./survival-lib')
const h = make('DataWatch')
const { bot } = h
const X = 7400; const Y = 200; const Z = 0

let meta = []
let attrs = 0
bot._client.on('entity_metadata', (p) => { if (p.entityId === bot.entity?.id) meta.push(p.metadata.map(m => m.key).join('+')) })
bot._client.on('entity_update_attributes', (p) => { if (p.entityId === bot.entity?.id) attrs++ })
const take = () => { const s = `meta=${meta.length}[${[...new Set(meta)].sort().join(',')}] attrs=${attrs}`; meta = []; attrs = 0; return s }

h.start(async () => {
  await h.arena(X, Y, Z, 4)
  h.cmd(`gamemode survival ${bot.username}`)
  h.cmd(`effect clear ${bot.username}`)
  await h.ticks(40)
  take()
  await h.ticks(100)
  h.result('idle_100', take())

  for (let i = 0; i < 3; i++) {
    bot.setControlState('sneak', true)
    await h.ticks(10)
    bot.setControlState('sneak', false)
    await h.ticks(10)
  }
  h.result('sneak_3', take())

  h.cmd(`damage ${bot.username} 2 minecraft:generic`)
  await h.ticks(20)
  h.result('damage', take())

  h.cmd(`effect give ${bot.username} minecraft:speed 2 1`)
  await h.ticks(20)
  h.result('effect_give', take())
  await h.ticks(60)
  h.result('effect_end', take())
})
