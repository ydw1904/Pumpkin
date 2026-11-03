// Maps: zooming and locking in a cartography table and extending in a crafting table make a new
// map with scaled or locked data once the result is taken; a map without data offers nothing.
// The maps' data is read from the map packets the bot gets while holding each map.
const { make } = require('./survival-lib')
const h = make('Mapper')
const { bot } = h
const X = 3100; const Y = 200; const Z = 0
const BX = X + 2; const BY = Y + 1; const BZ = Z

const maps = {}
bot._client.on('map', (p) => { maps[p.itemDamage] = `scale=${p.scale},locked=${p.locked}` })
// Known maps relative to the first one, so differently numbered worlds compare equal.
let first
const known = () => Object.keys(maps).map(Number).sort((a, b) => a - b).map((id) => `${id - first}:${maps[id]}`).join(' ')

const place = async (block) => {
  for (let i = 0; i < 5; i++) {
    h.cmd(`setblock ${BX} ${BY} ${BZ} air`)
    h.cmd(`setblock ${BX} ${BY} ${BZ} ${block}`)
    if (await h.waitFor(() => h.blockName(BX, BY, BZ) === block, 2000)) break
  }
  await h.ticks(2)
}
const open = async () => {
  const w = await bot.openBlock(h.block(BX, BY, BZ))
  await h.ticks(2)
  await bot._syncWindow(w)
  return w
}
const close = async () => { if (bot.currentWindow) bot.closeWindow(bot.currentWindow); await h.ticks(5) }
const invSlot = (w, i) => w.slots.length - 36 + (i < 9 ? 27 + i : i - 9)
const findIn = (w, name) => w.slots.findIndex((it, i) => i >= w.slots.length - 36 && it && it.name === name)
// Holds every filled map in the hotbar for a while so the server sends its data.
const holdMaps = async () => {
  for (let i = 0; i < 9; i++) {
    if (bot.inventory.slots[36 + i]?.name !== 'filled_map') continue
    bot.setQuickBarSlot(i)
    await h.ticks(25)
  }
}
const comps = (it) => it ? `${h.stackStr(it)}${JSON.stringify(it.componentMap ? [...it.componentMap.keys()].sort() : [])}` : 'empty'

// Puts the newest filled map and `extra` into a cartography table and shift-clicks the result.
const cartography = async (key, extra) => {
  await place('cartography_table')
  h.cmd(`item replace entity ${bot.username} container.20 with ${extra}`)
  await h.waitFor(() => bot.inventory.slots[20], 3000)
  const w = await open()
  const map = findIn(w, 'filled_map')
  await h.click(map, 0, 0)
  await h.click(0, 1, 0)
  await h.click(map, 0, 0)
  await h.click(invSlot(w, 20), 0, 0)
  await h.click(1, 1, 0)
  await h.click(invSlot(w, 20), 0, 0)
  await bot._syncWindow(w)
  h.result(`${key}.offer`, comps(w.slots[2]))
  await h.click(2, 0, 1)
  await close()
  h.cmd(`clear ${bot.username} ${extra.split(' ')[0]}`)
  await holdMaps()
  h.result(`${key}.maps`, known())
}

h.start(async () => {
  await h.arena(X, Y, Z, 4)
  await h.fill({ 0: 'map 1' })
  bot.setQuickBarSlot(0)
  await h.ticks(2)
  bot.activateItem()
  await h.waitFor(() => Object.keys(maps).length > 0, 5000)
  first = Math.min(...Object.keys(maps).map(Number))
  h.result('made', known())

  await cartography('zoom', 'paper 1')

  // Extending: the zoomed map in the middle of eight paper in a crafting table.
  await place('crafting_table')
  h.cmd(`item replace entity ${bot.username} container.20 with paper 8`)
  await h.waitFor(() => bot.inventory.slots[20], 3000)
  const w = await open()
  const map = findIn(w, 'filled_map')
  await h.click(map, 0, 0)
  await h.click(5, 1, 0)
  await h.click(map, 0, 0)
  await h.click(invSlot(w, 20), 0, 0)
  for (const slot of [1, 2, 3, 4, 6, 7, 8, 9]) await h.click(slot, 1, 0)
  await bot._syncWindow(w)
  h.result('extend.offer', comps(w.slots[0]))
  await h.click(0, 0, 1)
  await close()
  await holdMaps()
  h.result('extend.maps', known())

  await cartography('lock', 'glass_pane 1')

  // A map id without data offers nothing.
  h.cmd(`clear ${bot.username} filled_map`)
  h.cmd(`item replace entity ${bot.username} container.3 with filled_map[map_id=900]`)
  await h.waitFor(() => bot.inventory.slots[39], 3000)
  await cartography('no_data', 'paper 1')
})
