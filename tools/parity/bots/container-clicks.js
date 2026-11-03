// Shift-click routing in every container and workstation menu, shift-crafting at a crafting table
// with remainders, ender chests shared between blocks, and shulker boxes keeping their contents
// when broken. Each click is answered with the whole window, so the RESULT lines show the server's
// outcome.
const { make } = require('./survival-lib')
const h = make('Boxer')
const { bot } = h
const X = 2300; const Y = 200; const Z = 0
const BX = X + 2; const BY = Y + 1; const BZ = Z

const place = async (block, x = BX, y = BY, z = BZ) => {
  const name = block.split('[')[0]
  for (let i = 0; i < 5; i++) {
    h.cmd(`setblock ${x} ${y} ${z} air`)
    h.cmd(`setblock ${x} ${y} ${z} ${block}`)
    if (await h.waitFor(() => h.blockName(x, y, z) === name, 2000)) break
  }
  await h.ticks(2)
}
const open = async () => {
  const w = await bot.openBlock(h.block(BX, BY, BZ))
  await h.ticks(2)
  await bot._syncWindow(w)
  return w
}
const close = async () => {
  if (bot.currentWindow) bot.closeWindow(bot.currentWindow)
  await h.ticks(5)
}

// Opens `block` with the inventory filled from `map`, shift-clicks each window slot in `clicks`
// (negative numbers count back from the end of the window), and reports the window after each
// group.
const route = async (key, block, map, groups) => {
  await close()
  await place(block)
  await h.fill(map)
  const w = await open()
  h.result(`${key}.type`, `${w.type}:${w.slots.length}`)
  let n = 0
  for (const clicks of groups) {
    for (const c of clicks) await h.click(c < 0 ? w.slots.length + c : c, 0, 1)
    h.result(`${key}.${n++}`, h.win())
  }
  await close()
}

h.start(async () => {
  await h.arena(X, Y, Z, 6)
  h.cmd(`effect give ${bot.username} resistance infinite 255 true`)

  // Player inventory starts at window size - 36, the hotbar at size - 9.
  await route('chest', 'chest[facing=west]', { 0: 'cobblestone 64', 1: 'cobblestone 10', 9: 'dirt 30' },
    [[-9, -8, -36], [0], [1, 2]])
  await route('barrel', 'barrel[facing=up]', { 0: 'cobblestone 64', 9: 'dirt 30', 10: 'shulker_box' },
    [[-9, -36, -35], [0]])
  await route('shulker', 'shulker_box', { 0: 'cobblestone 64', 9: 'shulker_box', 10: 'red_shulker_box', 11: 'dirt 5' },
    [[-9, -36, -35, -34], [0]])
  await route('hopper', 'hopper', { 0: 'cobblestone 64', 9: 'dirt 30' }, [[-9, -36], [0]])
  await route('dispenser', 'dispenser[facing=up]', { 0: 'cobblestone 64', 9: 'dirt 30' }, [[-9, -36], [0]])
  await route('dropper', 'dropper[facing=up]', { 0: 'cobblestone 64', 9: 'dirt 30' }, [[-9, -36], [0]])
  await route('furnace', 'furnace', { 0: 'raw_iron 10', 1: 'coal 5', 2: 'dirt 3', 3: 'oak_log 2', 4: 'lava_bucket', 5: 'bucket', 6: 'charcoal 2', 9: 'stone 4', 10: 'raw_gold 3' },
    [[-9], [-8], [-7], [-6], [-5], [-4], [-36], [-35], [0, 1]])
  await route('blast_furnace', 'blast_furnace', { 0: 'raw_iron 10', 1: 'coal 5', 2: 'beef 3', 3: 'cobblestone 3' },
    [[-9, -8, -7, -6], [0, 1]])
  await route('smoker', 'smoker', { 0: 'raw_iron 10', 1: 'coal 5', 2: 'beef 3', 3: 'kelp 3' },
    [[-9, -8, -7, -6], [0, 1]])
  await route('brewing', 'brewing_stand', {
    0: 'potion[potion_contents={potion:"minecraft:water"}]', 1: 'potion[potion_contents={potion:"minecraft:water"}]',
    2: 'nether_wart 5', 3: 'blaze_powder 5', 4: 'blaze_powder 70', 5: 'glass_bottle 3', 6: 'dirt 2', 7: 'gunpowder 2', 8: 'splash_potion'
  }, [[-9, -8], [-7], [-6], [-5], [-4], [-3], [-2], [-1], [0, 3, 4]])
  await route('enchanting', 'enchanting_table', { 0: 'lapis_lazuli 70', 1: 'iron_sword', 2: 'dirt 5', 3: 'book 3' },
    [[-9], [-8], [-7], [-6], [0, 1]])
  await route('anvil', 'anvil', { 0: 'iron_sword', 1: 'dirt 5', 2: 'cobblestone 5' }, [[-9], [-8], [-7], [0]])
  await route('grindstone', 'grindstone[face=floor]', { 0: 'iron_sword', 1: 'dirt 5', 2: 'enchanted_book[stored_enchantments={sharpness:1}]', 3: 'golden_apple' },
    [[-9], [-8], [-7], [-6], [0]])
  await route('smithing', 'smithing_table', { 0: 'netherite_upgrade_smithing_template 2', 1: 'diamond_sword', 2: 'netherite_ingot 3', 3: 'dirt 5', 4: 'coast_armor_trim_smithing_template' },
    [[-9], [-8], [-7], [-6], [-5], [3], [0]])
  await route('stonecutter', 'stonecutter', { 0: 'stone 10', 1: 'dirt 5' }, [[-9], [-8], [0]])
  await route('loom', 'loom', { 0: 'white_banner 2', 1: 'red_dye 5', 2: 'creeper_banner_pattern', 3: 'dirt 5' },
    [[-9], [-8], [-7], [-6], [0]])
  await route('cartography', 'cartography_table', { 0: 'map 2', 1: 'paper 5', 2: 'glass_pane 2', 3: 'dirt 5' },
    [[-9], [-8], [-7], [-6], [0]])
  await route('crafting_table', 'crafting_table', { 0: 'oak_planks 64', 1: 'stick 3', 9: 'dirt 5' }, [[-9], [-8], [-36], [1]])
  // The crafter window ends with its result slot, so its inventory starts at 9 and the hotbar at 36.
  await route('crafter', 'crafter', { 0: 'oak_planks 64', 9: 'dirt 5' }, [[36], [9], [0]])
  // A switched-off crafter slot refuses items; the result slot shows what would be crafted.
  await close()
  await place('crafter')
  await h.fill({ 0: 'oak_planks 64' })
  const crafterProps = {}
  const onProp = (p) => { crafterProps[p.property] = p.value }
  bot._client.on('craft_progress_bar', onProp)
  const cw = await open()
  bot._client.write('set_slot_state', { slot_id: 4, window_id: cw.id, state: false })
  await h.ticks(5)
  await h.click(36, 0, 0); await h.click(4, 1, 0); await h.click(0, 1, 0); await h.click(36, 0, 0)
  await bot._syncWindow(cw)
  h.result('crafter.disabled_slot', `slot4_state=${crafterProps[4]} ${h.win(0, 9)} result=${h.stackStr(cw.slots[45])}`)
  bot._client.removeListener('craft_progress_bar', onProp)
  await close()

  // Shift-crafting at a crafting table: many stackables, non-stackables, and a recipe with remainders.
  const craft = async (key, map, layout, clicks) => {
    await close()
    await place('crafting_table')
    await h.fill(map)
    await open()
    for (const [from, to, button] of layout) { await h.click(from, 0, 0); await h.click(to, button, 0); await h.click(from, 0, 0) }
    h.result(`${key}.grid`, h.win(0, 10))
    for (const c of clicks) await h.click(...c)
    h.result(key, h.win())
    await close()
    h.result(`${key}.closed`, h.win())
  }
  // Inventory 0 is window slot 37, inventory 9 is window slot 10.
  await craft('craft_torches', { 0: 'coal 20', 1: 'stick 30' }, [[37, 1, 0], [38, 4, 0]], [[0, 0, 1]])
  await craft('craft_pickaxes', { 0: 'oak_planks 9', 1: 'stick 6' },
    [[37, 1, 1], [37, 2, 1], [37, 3, 1], [38, 5, 1], [38, 8, 1], [37, 1, 1], [37, 2, 1], [37, 3, 1], [38, 5, 1], [38, 8, 1]], [[0, 0, 1]])
  await craft('craft_cake', { 0: 'milk_bucket', 1: 'milk_bucket', 2: 'milk_bucket', 3: 'sugar 2', 4: 'egg', 5: 'wheat 3' },
    [[37, 1, 0], [38, 2, 0], [39, 3, 0], [40, 4, 1], [40, 6, 1], [41, 5, 0], [42, 7, 1], [42, 8, 1], [42, 9, 1]], [[0, 0, 1]])
  await craft('craft_dyed_armor', { 0: 'leather_chestplate', 1: 'red_dye' }, [[37, 1, 0], [38, 2, 0]], [[0, 0, 1]])
  await craft('craft_repair', { 0: 'iron_pickaxe[damage=200]', 1: 'iron_pickaxe[damage=100,enchantments={efficiency:3}]' }, [[37, 1, 0], [38, 2, 0]], [[0, 0, 1]])
  await craft('craft_map_clone', { 0: 'filled_map[map_id=7]', 1: 'map 3' }, [[37, 1, 0], [38, 2, 1], [38, 3, 1]], [[0, 0, 1]])
  await craft('craft_book_clone', { 0: 'written_book[written_book_content={title:"t",author:"a",pages:["p"]}]', 1: 'writable_book 2' },
    [[37, 1, 0], [38, 2, 0]], [[0, 0, 1]])
  await craft('craft_firework', { 0: 'paper 3', 1: 'gunpowder 6' }, [[37, 1, 0], [38, 2, 1], [38, 3, 1]], [[0, 0, 1]])
  await craft('craft_banner_copy', { 0: 'red_banner[banner_patterns=[{pattern:"minecraft:creeper",color:"black"}]]', 1: 'red_banner' },
    [[37, 1, 0], [38, 2, 0]], [[0, 0, 1]])
  if (bot.currentWindow) await close()
  // Dyed armor, repaired tools and map/book copies differ in components the names above don't show.
  const comps = () => bot.inventory.items().map(it => `${it.name}${JSON.stringify((it.componentMap && [...it.componentMap.keys()].sort()) || [])}`).sort().join(' ')
  await craft('craft_dyed_armor_components', { 0: 'leather_chestplate', 1: 'red_dye' }, [[37, 1, 0], [38, 2, 0]], [[0, 0, 1]])
  h.result('dyed_armor_components', comps())

  // Ender chests: one inventory per player, shared between blocks.
  await close()
  h.cmd(`item replace entity ${bot.username} enderchest.3 with diamond 2`)
  await place('ender_chest[facing=west]')
  await h.fill({ 0: 'gold_ingot 5' })
  let w = await open()
  h.result('ender.type', `${w.type}:${w.slots.length}`)
  await h.click(w.slots.length - 9, 0, 1)
  h.result('ender.first', h.win())
  await close()
  await place('ender_chest[facing=west]', BX, BY, BZ + 2)
  w = await bot.openBlock(h.block(BX, BY, BZ + 2))
  await h.ticks(2)
  await bot._syncWindow(w)
  h.result('ender.second', h.win())
  await close()
  h.cmd(`setblock ${BX} ${BY} ${BZ + 2} air`)

  // Shulker boxes keep their contents when broken, chests drop theirs.
  await place('shulker_box')
  await h.fill({ 0: 'cobblestone 64', 1: 'dirt 5' })
  await open()
  await h.click(-9 + 63, 0, 1)
  await h.click(-8 + 63, 0, 1)
  await close()
  await h.clear()
  h.cmd(`gamemode survival ${bot.username}`)
  await bot.dig(h.block(BX, BY, BZ), true)
  await h.ticks(20)
  // The bot may already have picked the drop up.
  const box = Object.values(bot.entities).filter(e => e.name === 'item' && e.getDroppedItem()).map(e => e.getDroppedItem())
    .concat(bot.inventory.items())
  h.result('shulker_drop', box.filter(it => it.name.endsWith('shulker_box')).map(it => `${h.stackStr(it)}${JSON.stringify((it.componentMap && [...it.componentMap.keys()].sort()) || [])}`).join(' ') || 'none')
  h.cmd('kill @e[type=item]')
  await place('chest[facing=west]')
  await h.fill({ 0: 'cobblestone 64', 1: 'dirt 5' })
  await open()
  await h.click(-9 + 63, 0, 1)
  await h.click(-8 + 63, 0, 1)
  await close()
  await bot.dig(h.block(BX, BY, BZ), true)
  await h.ticks(20)
  // Totals per item: how the contents split into entities is random.
  const dropped = {}
  for (const e of Object.values(bot.entities)) {
    const it = e.name === 'item' && e.getDroppedItem()
    if (it && ['chest', 'cobblestone', 'dirt'].includes(it.name)) dropped[it.name] = (dropped[it.name] || 0) + it.count
  }
  for (const it of bot.inventory.items()) dropped[it.name] = (dropped[it.name] || 0) + it.count
  h.result('chest_drop', Object.keys(dropped).sort().map(k => `${k}*${dropped[k]}`).join(',') || 'none')
  h.cmd('kill @e[type=item]')
})
