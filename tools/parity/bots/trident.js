// A thrown Loyalty trident hurts a husk, grants "A Throwaway Joke" and flies back to its thrower;
// a Riptide trident used in water starts the thrower's spin attack, seen by a second player.
const mineflayer = require('mineflayer')
const { make } = require('./survival-lib')
const h = make('Tridenter')
const { bot } = h
const X = 0; const Y = 200; const Z = 0

const achieved = new Set()
bot._client.on('advancements', (packet) => {
  for (const entry of packet.progressMapping || []) {
    if ((entry.value || []).some((c) => c.criterionProgress !== undefined && c.criterionProgress !== null)) achieved.add(entry.key)
  }
})

h.start(async () => {
  await h.arena(X, Y, Z, 6)
  h.cmd('difficulty normal')
  h.cmd('weather clear')
  h.cmd(`summon husk ${X + 0.5} ${Y + 1} ${Z + 4.5} {NoAI:1b,Silent:1b,PersistenceRequired:1b}`)
  await h.waitFor(() => Object.values(bot.entities).some((e) => e.name === 'husk'), 5000)
  const husk = Object.values(bot.entities).find((e) => e.name === 'husk')
  await h.clear()
  await h.give('trident[enchantments={"minecraft:loyalty":3}]')
  const trident = bot.inventory.items().find((i) => i.name === 'trident')
  h.result('has_trident', !!trident)
  if (husk && trident) {
    await bot.equip(trident, 'hand')
    await bot.lookAt(husk.position.offset(0, 1.0, 0), true)
    await h.ticks(5)
    // The throw sound follows the thrown trident.
    const entitySounds = []
    const onEntitySound = (p) => entitySounds.push(p)
    bot._client.on('entity_sound_effect', onEntitySound)
    bot.activateItem()
    await h.sleep(1000)
    bot.deactivateItem()
    await h.waitFor(() => !bot.inventory.items().some((i) => i.name === 'trident'), 3000)
    h.result('thrown', !bot.inventory.items().some((i) => i.name === 'trident'))
    await h.ticks(2)
    bot._client.removeListener('entity_sound_effect', onEntitySound)
    const thrownTrident = Object.values(bot.entities).find((e) => e.name === 'trident')
    h.result('throw_sound', entitySounds.filter((p) => thrownTrident && p.entityId === thrownTrident.id)
      .map((p) => `${JSON.stringify(p.sound)}/${p.volume}/${p.pitch}`).join(',') || 'none')
    const returned = await h.waitFor(() => bot.inventory.items().some((i) => i.name === 'trident'), 10000)
    h.result('loyalty_returned', returned)
    await h.ticks(10)
    h.result('throw_trident_advancement', achieved.has('minecraft:adventure/throw_trident'))
    h.result('shoot_arrow_advancement', achieved.has('minecraft:adventure/shoot_arrow'))
  }
  h.cmd('kill @e[type=husk]')

  // Riptide in water: the watcher sees the thrower's living flags turn on the riptide bit.
  const watcher = mineflayer.createBot({ host: '127.0.0.1', port: Number(process.env.PORT), username: 'Watcher', version: '26.3', auth: 'offline' })
  await new Promise((resolve) => watcher.once('spawn', resolve))
  h.cmd('gamemode spectator Watcher')
  h.cmd(`tp Watcher ${X + 4.5} ${Y + 3} ${Z + 0.5}`)
  h.cmd(`fill ${X - 2} ${Y + 1} ${Z - 2} ${X + 2} ${Y + 1} ${Z + 2} water`)
  await h.sleep(2000)
  let riptideSeen = false
  watcher._client.on('entity_metadata', (packet) => {
    const self = watcher.players.Tridenter && watcher.players.Tridenter.entity
    if (!self || packet.entityId !== self.id) return
    for (const item of packet.metadata || []) {
      if (item.key === 8 && typeof item.value === 'number' && (item.value & 4) !== 0) riptideSeen = true
    }
  })
  await h.clear()
  await h.give('trident[enchantments={"minecraft:riptide":3}]')
  const riptide = bot.inventory.items().find((i) => i.name === 'trident')
  if (riptide) {
    await bot.equip(riptide, 'hand')
    await bot.look(0, 0.3, true)
    bot.activateItem()
    await h.sleep(1000)
    bot.deactivateItem()
    await h.waitFor(() => riptideSeen, 3000)
  }
  h.result('riptide_flag_seen_by_watcher', riptideSeen)
  const used = bot.inventory.items().find((i) => i.name === 'trident')
  h.result('riptide_trident_kept', !!used)
  watcher.quit()
  h.cmd(`fill ${X - 2} ${Y + 1} ${Z - 2} ${X + 2} ${Y + 1} ${Z + 2} air`)

  // A trident summoned with arrow NBT shows its crit flag and pierce level.
  const summonedMeta = new Map()
  bot._client.on('entity_metadata', (packet) => {
    const e = bot.entities[packet.entityId]
    if (!e || e.name !== 'trident') return
    for (const item of packet.metadata || []) summonedMeta.set(item.key, item.value)
  })
  h.cmd('kill @e[type=trident]')
  await h.ticks(5)
  h.cmd(`summon trident ${X + 0.5} ${Y + 3} ${Z + 3.5} {NoGravity:1b,crit:1b,PierceLevel:3b}`)
  await h.waitFor(() => summonedMeta.size > 0, 3000)
  await h.ticks(5)
  h.result('summoned_nbt_meta', [...summonedMeta.keys()].sort((a, b) => a - b).map((k) => `${k}=${JSON.stringify(summonedMeta.get(k))}`).join(' ') || 'none')
})
