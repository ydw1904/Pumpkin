// Chest loot with item components: a shipwreck map chest gives a buried treasure map that leads
// to the nearest buried treasure, and an outpost chest gives enchanted books and a goat horn with
// an instrument. A plain map held in the hand fills in around the player column by column.
const { make } = require('./survival-lib')
const h = make('Treasure')
const { bot } = h
const X = 0; const Y = 200; const Z = 0

const maps = {}
bot._client.on('map', (p) => {
  const m = maps[p.itemDamage] || (maps[p.itemDamage] = { colors: Buffer.alloc(128 * 128), packets: 0 })
  m.packets++
  if (p.icons) m.icons = p.icons.map((i) => `${i.type}@${i.x},${i.z}r${i.direction}`).sort().join(' ')
  if (p.columns > 0 && p.data) {
    for (let x = 0; x < p.columns; x++) for (let y = 0; y < p.rows; y++) m.colors[p.x + x + (p.y + y) * 128] = p.data[x + y * p.columns]
  }
})
const comps = (it) => it.componentMap ? [...it.componentMap.keys()].sort().join(',') : ''
const decorations = (it) => {
  const c = it.componentMap && it.componentMap.get('map_decorations')
  const value = c && c.data && c.data.value
  return value ? Object.keys(value).sort().map((k) => {
    const d = value[k].value
    return `${k}:${d.type.value}@${d.x.value},${d.z.value}r${d.rotation.value}`
  }).join(' ') : 'none'
}
const colorHash = (m) => {
  let hash = 0
  for (const c of m.colors) hash = (hash * 31 + c) >>> 0
  return hash.toString(16)
}

h.start(async () => {
  await h.arena(X, Y, Z, 6)
  await h.clear()
  h.cmd(`execute as ${bot.username} at @s run loot give @s loot minecraft:chests/shipwreck_map`)
  await h.waitFor(() => bot.inventory.items().some((it) => it.name === 'buried_treasure_map'), 20000)
  await h.ticks(10)
  const treasure = bot.inventory.items().find((it) => it.name === 'buried_treasure_map')
  h.result('treasure_map', treasure ? `${comps(treasure)} ${decorations(treasure)}` : 'none')
  if (treasure) {
    await bot.equip(treasure, 'hand')
    await h.ticks(40)
    const shown = Object.values(maps).map((m) => `icons=[${m.icons}] painted=${m.colors.filter((c) => c !== 0).length} hash=${colorHash(m)}`)
    h.result('treasure_map_data', shown.join(' | ') || 'none')
  }

  // Loot rolls differ between servers, so only what the components look like is compared.
  await h.clear()
  for (let i = 0; i < 8; i++) h.cmd(`execute as ${bot.username} at @s run loot give @s loot minecraft:chests/pillager_outpost`)
  await h.waitFor(() => bot.inventory.items().some((it) => it.name === 'goat_horn'), 10000)
  await h.ticks(10)
  const kinds = (names) => [...new Set(bot.inventory.items().filter((it) => names.includes(it.name)).map((it) => `${it.name}[${comps(it)}]`))].sort().join(' ') || 'none'
  h.result('outpost_books', kinds(['book', 'enchanted_book']))
  h.result('outpost_horns', kinds(['goat_horn']))
  h.result('outpost_crossbows', kinds(['crossbow']))

  // A fresh map held for a while: the first packets carry the explored columns only.
  await h.clear()
  for (const id of Object.keys(maps)) delete maps[id]
  await h.fill({ 0: 'map 1' })
  bot.setQuickBarSlot(0)
  await h.ticks(2)
  bot.activateItem()
  await h.waitFor(() => Object.keys(maps).length > 0, 5000)
  await h.ticks(1)
  const first = Object.values(maps)[0]
  h.result('fresh_map_first_tick', first ? `painted=${first.colors.filter((c) => c !== 0).length}` : 'none')
  await h.ticks(40)
  const held = Object.values(maps)[0]
  h.result('fresh_map_after_40_ticks', held ? `icons=[${held.icons}] painted=${held.colors.filter((c) => c !== 0).length} hash=${colorHash(held)}` : 'none')
})
