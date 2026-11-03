// Recipe book: the displays sent for dye, imbue and transmute recipes, and what placing recipes
// from the book puts into the crafting table, the inventory grid and a furnace (centering, max
// placement, ghost recipes for what can't be filled, the furnace result moved out first).
// Display ids and groups differ between servers, so recipes are looked up by what they show.
const { make } = require('./survival-lib')
const h = make('Book')
const { bot } = h
const X = 2900; const Y = 200; const Z = 0
const BX = X + 2; const BY = Y + 1; const BZ = Z

const entries = new Map()
bot._client.on('recipe_book_add', (p) => {
  if (p.replace) entries.clear()
  for (const e of p.entries) entries.set(e.recipe.displayId, e.recipe)
})
let ghosts = 0
bot._client.on('craft_recipe_response', () => { ghosts++ })

const name = (id) => bot.registry.items[id]?.name ?? `#${id}`
// Ingredients of several items read as `any`: vanilla sends every ingredient as a holder set,
// Pumpkin single items and lists of them.
const desc = (sd) => {
  if (!sd) return '?'
  const d = sd.data
  switch (sd.type) {
    case 'empty': return '_'
    case 'any_fuel': return 'fuel'
    case 'item': return name(d)
    case 'item_stack': return `${name(d.itemId)}*${d.itemCount}`
    case 'tag': return d.ids?.length === 1 ? name(d.ids[0]) : 'any'
    case 'composite': return d.length === 1 ? desc(d[0]) : 'any'
    case 'with_any_potion': return `potion(${desc(d.base)})`
    case 'only_with_component': return `with(${desc(d.source)})`
    case 'dyed_slot_demo': return `dyed(${desc(d.dye)},${desc(d.target)})`
    default: return sd.type
  }
}
const resultOf = (r) => desc(r.display.data.result)
const show = (r) => {
  const d = r.display.data
  const ingredients = (d.ingredients || [d.ingredient]).map(desc).join(' ')
  const size = d.width ? `${d.width}x${d.height} ` : ''
  return `${r.display.type} ${size}[${ingredients}] -> ${resultOf(r)} at ${desc(d.craftingStation)} ${r.category} needs=${r.craftingRequirements?.length ?? 'none'}`
}
const find = (pred) => [...entries.entries()].filter(([, r]) => pred(r))

const place = async (block, x = BX, y = BY, z = BZ) => {
  const base = block.split('[')[0]
  for (let i = 0; i < 5; i++) {
    h.cmd(`setblock ${x} ${y} ${z} air`)
    h.cmd(`setblock ${x} ${y} ${z} ${block}`)
    if (await h.waitFor(() => h.blockName(x, y, z) === base, 2000)) break
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
const placeRecipe = async (displayId, makeAll = false) => {
  const window = bot.currentWindow || bot.inventory
  bot._client.write('craft_recipe_request', { windowId: window.id, recipeId: displayId, makeAll })
  await h.ticks(4)
  await bot._syncWindow(window)
}
// Places the recipe found by `pred` into the open window and reports the window and inventory.
const placeAndShow = async (key, pred, makeAll = false, gridSlots = 10) => {
  const found = find(pred)
  if (found.length === 0) { h.result(key, 'no display'); return }
  const before = ghosts
  await placeRecipe(found[0][0], makeAll)
  h.result(key, `grid=${h.win(0, gridSlots)} inv=${h.invStr()} ghost=${ghosts - before}`)
}

h.start(async () => {
  await h.arena(X, Y, Z, 4)
  h.cmd(`recipe give ${bot.username} *`)
  await h.waitFor(() => entries.size > 1000, 10000)
  await h.ticks(5)

  for (const [key, pred] of [
    ['dye', (r) => show(r).includes('dyed')],
    ['imbue_tipped_arrow', (r) => resultOf(r).includes('tipped_arrow')],
    ['transmute_red_shulker_box', (r) => resultOf(r).startsWith('red_shulker_box')],
    ['map', (r) => show(r).includes('filled_map')]
  ]) {
    h.result(`display.${key}`, find(pred).map(([, r]) => show(r)).sort().join(' | ') || 'none')
  }

  const planks = (r) => r.display.type === 'crafting_shapeless' && resultOf(r) === 'oak_planks*4'
  const sticks = (r) => r.display.type === 'crafting_shaped' && resultOf(r) === 'stick*4' && !show(r).includes('bamboo')

  // The 2x2 grid of the inventory, left aligned.
  await h.fill({ 0: 'oak_log 2' })
  await placeAndShow('place.inventory_planks', planks, false, 5)
  await placeAndShow('place.inventory_planks_again', planks, false, 5)

  // A crafting table: a 1x1 recipe goes into the middle, a 1x2 one into the middle column.
  await place('crafting_table')
  await h.fill({ 0: 'oak_log 3' })
  await open()
  await placeAndShow('place.table_planks', planks)
  await placeAndShow('place.table_planks_max', planks, true)
  await close()
  await h.fill({ 0: 'oak_planks 5', 1: 'oak_planks[custom_name="named"] 4' })
  await open()
  await placeAndShow('place.table_sticks_max', sticks, true)
  await close()

  // Dyeing, imbuing and transmuting from the book.
  await h.fill({ 0: 'leather_helmet', 1: 'red_dye 3' })
  await open()
  await placeAndShow('place.dye', (r) => show(r).includes('dyed') && show(r).includes('leather_helmet'))
  await close()
  await h.fill({ 0: 'arrow 20', 1: 'lingering_potion[potion_contents={potion:"minecraft:swiftness"}]' })
  await open()
  await placeAndShow('place.imbue', (r) => resultOf(r).includes('tipped_arrow'))
  await close()
  await h.fill({ 0: 'white_shulker_box', 1: 'red_dye 2' })
  await open()
  await placeAndShow('place.transmute', (r) => resultOf(r).startsWith('red_shulker_box'))
  await close()
  // Map cloning takes up to eight maps, and placing it needs all eight.
  await h.fill({ 0: 'filled_map[map_id=0]', 1: 'map 2' })
  await open()
  await placeAndShow('place.map_cloning_short', (r) => resultOf(r) === 'filled_map*2')
  await close()
  await h.fill({ 0: 'filled_map[map_id=0]', 1: 'map 9' })
  await open()
  await placeAndShow('place.map_cloning', (r) => resultOf(r) === 'filled_map*2')
  await close()
  // Missing ingredients clear the grid and show a ghost recipe.
  await h.fill({ 0: 'stick 1' })
  await open()
  await placeAndShow('place.missing', (r) => resultOf(r) === 'oak_planks*4' && r.display.type === 'crafting_shapeless')
  await close()

  // A furnace: the input goes into the top slot, and the result slot is emptied into the
  // inventory first.
  await place('furnace[facing=north]')
  h.cmd(`item replace block ${BX} ${BY} ${BZ} container.2 with gold_ingot 2`)
  await h.fill({ 0: 'raw_iron 64', 1: 'raw_iron 6' })
  await open()
  const ironFromRaw = (r) => r.display.type === 'furnace' && resultOf(r) === 'iron_ingot*1' && desc(r.display.data.ingredient) === 'raw_iron' && desc(r.display.data.craftingStation) === 'furnace'
  await placeAndShow('place.furnace', ironFromRaw, false, 3)
  await placeAndShow('place.furnace_max', ironFromRaw, true, 3)
  await close()
})
