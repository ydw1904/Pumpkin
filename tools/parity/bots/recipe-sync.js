// Recipe sync: ClientboundUpdateRecipesPacket at login and after a reload (where it falls among
// the tags and the recipe book), its recipe property sets and stonecutter recipes, datapack
// recipes replacing and adding to them, and the server's stonecutter buttons and furnace
// shift-click following the same recipes. Property sets are hash sets in vanilla, so they are
// compared sorted; the stonecutter list is in registry order on both, so its bytes are compared.
const crypto = require('crypto')
const fs = require('fs')
const path = require('path')
const { make } = require('./survival-lib')
const h = make('Cutter')
const { bot } = h
const X = 7200; const Y = 200; const Z = 0
const BX = X + 2; const BY = Y + 1; const BZ = Z

const watched = ['login', 'held_item_slot', 'tags', 'declare_recipes', 'recipe_book_settings', 'recipe_book_add']
const order = []
for (const name of watched) {
  bot._client.on(name, () => { if (order[order.length - 1] !== name) order.push(name) })
}
// The raw and the decoded packet are emitted separately, so they are paired by index.
const raws = []
const decoded = []
bot._client.on('raw.declare_recipes', (buffer) => { raws.push(buffer) })
bot._client.on('declare_recipes', (p) => { decoded.push(p) })
const packets = { get length () { return Math.min(raws.length, decoded.length) } }
const packet = (i) => ({ raw: raws[i], decoded: decoded[i] })

// The bytes of the stonecutter list: what follows the property sets.
const stonecutterBytes = (raw) => {
  let offset = 0
  const varint = () => {
    let value = 0; let shift = 0; let b
    do { b = raw[offset++]; value |= (b & 0x7f) << shift; shift += 7 } while (b & 0x80)
    return value
  }
  varint() // packet id
  const sets = varint()
  for (let i = 0; i < sets; i++) {
    const keyLength = varint()
    offset += keyLength
    const n = varint()
    for (let j = 0; j < n; j++) varint()
  }
  return raw.subarray(offset)
}
const name = (id) => bot.registry.items[id]?.name ?? `#${id}`
const input = (set) => set.name !== undefined ? `#${set.name}` : set.ids.map(name).join('|')
const display = (sd) => sd.type === 'item_stack' ? `${name(sd.data.itemId)}*${sd.data.itemCount}` : sd.type
// Stonecutter results grouped by input, in list order.
const byInput = (p) => {
  const groups = new Map()
  for (const r of p.stoneCutterRecipes) {
    const key = input(r.input)
    if (!groups.has(key)) groups.set(key, [])
    groups.get(key).push(display(r.slotDisplay))
  }
  return groups
}
const report = (prefix, packet, inputs) => {
  const p = packet.decoded
  for (const set of [...p.recipes].sort((a, b) => a.name.localeCompare(b.name))) {
    h.result(`${prefix}.set.${set.name}`, `${set.items.length} ${set.items.map(name).sort().join(',')}`)
  }
  h.result(`${prefix}.stonecutter_count`, p.stoneCutterRecipes.length)
  h.result(`${prefix}.stonecutter_sha1`, crypto.createHash('sha1').update(stonecutterBytes(packet.raw)).digest('hex'))
  const groups = byInput(p)
  for (const [key, results] of groups) {
    if (!inputs || inputs.includes(key)) h.result(`${prefix}.cut.${key}`, results.join(','))
  }
}

const place = async (block) => {
  for (let i = 0; i < 5; i++) {
    h.cmd(`setblock ${BX} ${BY} ${BZ} air`)
    h.cmd(`setblock ${BX} ${BY} ${BZ} ${block}`)
    if (await h.waitFor(() => h.blockName(BX, BY, BZ) === block, 2000)) break
  }
  await h.ticks(2)
}
const open = async (block, map) => {
  if (bot.currentWindow) { bot.closeWindow(bot.currentWindow); await h.ticks(5) }
  await place(block)
  await h.fill(map)
  const w = await bot.openBlock(h.block(BX, BY, BZ))
  await h.ticks(2)
  await bot._syncWindow(w)
  return w
}
const close = async () => { if (bot.currentWindow) bot.closeWindow(bot.currentWindow); await h.ticks(5) }
const inv = (w, i) => w.slots.length - 36 + (i < 9 ? 27 + i : i - 9)
const button = async (id) => {
  bot._client.write('enchant_item', { windowId: bot.currentWindow.id, enchantment: id })
  await h.ticks(3)
  await bot._syncWindow(bot.currentWindow)
}
// Clicks every stonecutter button for `item` and compares the results with what the packet
// lists for inputs naming the item directly, which is what the client shows.
const cutter = async (key, item) => {
  const listed = packet(packets.length - 1).decoded.stoneCutterRecipes
    .filter(r => r.input.ids && r.input.ids.map(name).includes(item))
    .map(r => display(r.slotDisplay))
  const w = await open('stonecutter', { 0: `${item} 10` })
  await h.click(inv(w, 0), 0, 0)
  await h.click(0, 0, 0)
  const cut = []
  // One button past the list: vanilla ignores it and keeps the last result.
  for (let i = 0; i <= Math.max(listed.length, 1); i++) {
    await button(i)
    cut.push(h.stackStr(w.slots[1]))
  }
  h.result(`${key}.buttons`, cut.join(','))
  if (listed.length > 0) h.result(`${key}.matches_packet`, cut.slice(0, listed.length).join(',') === listed.join(','))
  await close()
}
// Shift-clicks `item` from the first hotbar slot of a furnace-like block.
const shiftIntoFurnace = async (key, block, item) => {
  const w = await open(block, { 0: `${item} 1` })
  await h.click(inv(w, 0), 0, 1)
  await bot._syncWindow(w)
  h.result(key, h.win(0, 3) + ' inv=' + h.invStr())
  await close()
}

h.start(async () => {
  await h.ticks(20)
  const settings = order.indexOf('recipe_book_settings')
  h.result('login.order', order.slice(0, settings + 1).filter(n => n !== 'recipe_book_add').join(','))
  // Recipes awarded before the recipe book is sent.
  h.result('login.book_add_before_settings', order.slice(0, settings).includes('recipe_book_add'))
  h.result('login.packets', packets.length)
  if (packets.length === 0) return
  report('login', packet(0))

  await h.arena(X, Y, Z, 4)
  await cutter('cut_stone', 'stone')
  await shiftIntoFurnace('furnace_dirt_before', 'furnace', 'dirt')
  await shiftIntoFurnace('furnace_raw_iron', 'furnace', 'raw_iron')
  await shiftIntoFurnace('smoker_raw_iron', 'smoker', 'raw_iron')

  // A datapack replacing a built-in stonecutter recipe, adding one with an item and one with a
  // tag, and a smelting recipe that makes dirt a furnace input.
  const pack = path.join(path.dirname(process.env.CMDS), 'world', 'datapacks', 'parity_rs')
  const packFile = (rel, json) => {
    fs.mkdirSync(path.dirname(path.join(pack, 'data', ...rel)), { recursive: true })
    fs.writeFileSync(path.join(pack, 'data', ...rel), JSON.stringify(json))
  }
  fs.mkdirSync(pack, { recursive: true })
  fs.writeFileSync(path.join(pack, 'pack.mcmeta'), JSON.stringify({ pack: { description: 'recipe sync', min_format: 121, max_format: 121 } }))
  packFile(['minecraft', 'recipe', 'stone_slab_from_stone_stonecutting.json'], {
    type: 'minecraft:stonecutting', ingredient: 'minecraft:stone', result: { id: 'minecraft:diamond', count: 3 }
  })
  packFile(['p', 'recipe', 'stone_to_gold.json'], {
    type: 'minecraft:stonecutting', ingredient: 'minecraft:stone', result: { id: 'minecraft:gold_block' }
  })
  packFile(['p', 'recipe', 'logs_to_sticks.json'], {
    type: 'minecraft:stonecutting', ingredient: '#minecraft:logs', result: { id: 'minecraft:stick', count: 4 }
  })
  packFile(['p', 'recipe', 'dirt_smelting.json'], {
    type: 'minecraft:smelting', ingredient: 'minecraft:dirt', result: { id: 'minecraft:diamond' }, experience: 1, cookingtime: 200
  })
  // ReloadCommand.discoverNewPacks enables the new pack.
  const before = packets.length
  order.length = 0
  h.cmd('reload')
  await h.waitFor(() => packets.length > before, 20000)
  await h.ticks(20)
  h.result('reload.packets', packets.length - before)
  h.result('reload.order', order.filter(n => n !== 'held_item_slot').join(','))
  report('reload', packet(packets.length - 1), ['stone', '#minecraft:logs', 'dirt'])
  await cutter('cut_stone_dp', 'stone')
  await cutter('cut_oak_log_dp', 'oak_log')
  await shiftIntoFurnace('furnace_dirt_after', 'furnace', 'dirt')
  await shiftIntoFurnace('blast_dirt_after', 'blast_furnace', 'dirt')
})
