// Workstations: furnace-like cook and fuel times, remainders and XP; brewing fuel, time and splash
// conversion; enchanting costs with and without bookshelves and what an enchant costs; anvil
// rename, combine, repair and "too expensive"; grindstone; smithing upgrade and trims; stonecutter,
// loom and cartography results.
const { make } = require('./survival-lib')
const h = make('Smith')
const { bot } = h
const X = 2500; const Y = 200; const Z = 0
const BX = X + 2; const BY = Y + 1; const BZ = Z

const props = {}
bot._client.on('craft_progress_bar', (p) => { props[p.property] = p.value })
const propStr = (ids) => ids.map(i => props[i] ?? '-').join('/')

const place = async (block, x = BX, y = BY, z = BZ) => {
  const name = block.split('[')[0]
  for (let i = 0; i < 5; i++) {
    h.cmd(`setblock ${x} ${y} ${z} air`)
    h.cmd(`setblock ${x} ${y} ${z} ${block}`)
    if (await h.waitFor(() => h.blockName(x, y, z) === name, 2000)) break
  }
  await h.ticks(2)
}
const open = async (block, map) => {
  if (bot.currentWindow) { bot.closeWindow(bot.currentWindow); await h.ticks(5) }
  if (block) await place(block)
  if (map) await h.fill(map)
  for (const k of Object.keys(props)) delete props[k]
  const w = await bot.openBlock(h.block(BX, BY, BZ))
  await h.ticks(2)
  await bot._syncWindow(w)
  return w
}
const close = async () => { if (bot.currentWindow) bot.closeWindow(bot.currentWindow); await h.ticks(5) }
// Moves `button` (0 all, 1 one) from window slot `from` to `to` and puts the rest back.
const put = async (from, to, button = 0) => { await h.click(from, 0, 0); await h.click(to, button, 0); await h.click(from, 0, 0) }
const inv = (w, i) => w.slots.length - 36 + (i < 9 ? 27 + i : i - 9)
const button = async (id) => {
  bot._client.write('enchant_item', { windowId: bot.currentWindow.id, enchantment: id })
  await h.ticks(3)
  await bot._syncWindow(bot.currentWindow)
}
const comps = (it) => it ? `${h.stackStr(it)}${JSON.stringify(it.componentMap ? [...it.componentMap.keys()].sort() : [])}` : 'empty'
const setLevels = async (n) => {
  h.cmd(`xp set ${bot.username} 0 points`)
  h.cmd(`xp set ${bot.username} ${n} levels`)
  await h.waitFor(() => bot.experience.level === n, 3000)
}

h.start(async () => {
  await h.arena(X, Y, Z, 8)
  h.cmd(`effect give ${bot.username} resistance infinite 255 true`)

  // Furnace-like blocks: burn duration and total cook time properties, fuel remainders, output.
  const furnace = async (key, block, input, fuel, waitTicks) => {
    let w = await open(block, { 0: input, 1: fuel })
    await put(inv(w, 0), 0)
    await put(inv(w, 1), 1)
    await h.ticks(20)
    h.result(`${key}.props_after_20`, `lit_duration=${props[1]} total=${props[3]}`)
    if (waitTicks) {
      await h.serverTicks(waitTicks)
      await bot._syncWindow(w)
      h.result(`${key}.after_${waitTicks}_more`, h.win(0, 3))
    }
    return w
  }
  // Server ticks only advance in steps of 20 here, so the checks stay well clear of 200.
  await furnace('furnace_coal', 'furnace', 'raw_iron 3', 'coal 2', 120)
  await h.serverTicks(120)
  await bot._syncWindow(bot.currentWindow)
  h.result('furnace_coal.after_260', h.win(0, 3))
  await furnace('furnace_lava', 'furnace', 'cobblestone 1', 'lava_bucket', 0)
  await furnace('furnace_blaze_rod', 'furnace', 'cobblestone 1', 'blaze_rod 1', 0)
  await furnace('furnace_stick', 'furnace', 'cobblestone 1', 'stick 1', 0)
  await furnace('furnace_log', 'furnace', 'cobblestone 1', 'oak_log 1', 0)
  await furnace('furnace_kelp_block', 'furnace', 'cobblestone 1', 'dried_kelp_block 1', 0)
  await furnace('furnace_carpet', 'furnace', 'cobblestone 1', 'white_carpet 1', 0)
  await furnace('furnace_bamboo', 'furnace', 'cobblestone 1', 'bamboo 1', 0)
  await furnace('furnace_wooden_pickaxe', 'furnace', 'cobblestone 1', 'wooden_pickaxe 1', 0)
  await furnace('furnace_wet_sponge_bucket', 'furnace', 'wet_sponge 1', 'bucket 1', 0)
  await furnace('blast_raw_iron', 'blast_furnace', 'raw_iron 1', 'coal 1', 0)
  await furnace('blast_beef', 'blast_furnace', 'beef 1', 'coal 1', 0)
  await furnace('smoker_beef', 'smoker', 'beef 1', 'coal 1', 0)
  // XP: two raw gold give 1.0 each. Taking the output by shift-click pays out the stored XP.
  await setLevels(0)
  const w = await furnace('blast_xp', 'blast_furnace', 'raw_gold 2', 'coal 1', 240)
  await h.click(2, 0, 1)
  await h.ticks(40)
  h.result('blast_xp.taken', `${h.win(0, 3)} inv_gold=${h.inv().gold_ingot || 0} xp_points=${bot.experience.points} level=${bot.experience.level} orbs=${h.orbs().length}`)
  void w
  await close()

  // Brewing: blaze powder gives 20 fuel and one is used to start; brewing takes 400 ticks.
  const water = 'potion[potion_contents={potion:"minecraft:water"}]'
  let b = await open('brewing_stand', { 0: water, 1: water, 2: water, 3: 'nether_wart 2', 4: 'blaze_powder 2', 5: 'gunpowder 1' })
  for (let i = 0; i < 3; i++) await put(inv(b, i), i)
  await put(inv(b, 4), 4)
  await put(inv(b, 3), 3, 1)
  await h.ticks(20)
  h.result('brewing.props_after_20', `fuel=${props[1]} brew_time_ok=${props[0] >= 360 && props[0] <= 385}`)
  await h.serverTicks(300)
  await bot._syncWindow(b)
  h.result('brewing.after_320', h.win(0, 5))
  await h.serverTicks(140)
  await bot._syncWindow(b)
  h.result('brewing.after_460', `${h.win(0, 5)} ${b.slots.slice(0, 3).map(comps).join(' ')}`)
  await put(inv(b, 5), 3)
  await h.serverTicks(440)
  await bot._syncWindow(b)
  h.result('brewing.splash', `${h.win(0, 5)} fuel=${props[1]}`)
  await close()

  // Enchanting: costs with no bookshelves stay at or below 8; 15 bookshelves give slot 3 a cost of 30.
  b = await open('enchanting_table', { 0: 'iron_sword', 1: 'lapis_lazuli 10' })
  await put(inv(b, 0), 0)
  await put(inv(b, 1), 1)
  await h.ticks(5)
  // A slot costing less than its number is shown as 0 (off).
  h.result('enchant.no_shelves', `costs_le_8=${[0, 1, 2].every(i => props[i] >= 0 && props[i] <= 8)} seed_set=${props[3] !== undefined}`)
  await close()
  for (let dx = -2; dx <= 2; dx++) {
    for (let dz = -2; dz <= 2; dz++) {
      if (Math.max(Math.abs(dx), Math.abs(dz)) === 2 && !(dx === -2 && dz === 0)) h.cmd(`setblock ${BX + dx} ${BY} ${BZ + dz} bookshelf`)
    }
  }
  await h.ticks(5)
  await setLevels(30)
  b = await open(null, { 0: 'iron_sword', 1: 'lapis_lazuli 10', 2: 'book 1' })
  await put(inv(b, 0), 0)
  await put(inv(b, 1), 1)
  await h.ticks(5)
  // getEnchantmentCost with 15 shelves: slot 1 in 2..10, slot 2 in 6..21, slot 3 always 30.
  h.result('enchant.15_shelves', `in_range=${props[0] >= 2 && props[0] <= 10 && props[1] >= 6 && props[1] <= 21} slot3=${props[2]} has_clue=${props[4] >= 0}`)
  const seedBefore = props[3]
  await button(2)
  await h.ticks(5)
  h.result('enchant.after_slot3', `${h.win(0, 2)} level=${bot.experience.level} enchanted=${!!(b.slots[0] && b.slots[0].componentMap && b.slots[0].componentMap.has('enchantments'))} seed_changed=${props[3] !== seedBefore}`)
  // Not enough lapis or levels: the click does nothing.
  await h.click(0, 0, 1)
  await put(inv(b, 2), 0)
  await setLevels(2)
  await bot._syncWindow(b)
  await h.ticks(3)
  await button(2)
  h.result('enchant.too_poor', `${h.win(0, 2)} level=${bot.experience.level}`)
  // A book turns into an enchanted book with stored enchantments.
  await setLevels(30)
  await bot._syncWindow(b)
  await h.ticks(3)
  await button(0)
  h.result('enchant.book', `${comps(b.slots[0])} lapis=${h.stackStr(b.slots[1])} level=${bot.experience.level}`)
  await close()
  for (let dx = -2; dx <= 2; dx++) for (let dz = -2; dz <= 2; dz++) if (Math.max(Math.abs(dx), Math.abs(dz)) === 2) h.cmd(`setblock ${BX + dx} ${BY} ${BZ + dz} air`)

  // Anvil.
  const anvil = async (key, map, slots, name) => {
    await setLevels(50)
    const a = await open('anvil', map)
    for (const [i, s] of slots) await put(inv(a, i), s)
    if (name !== undefined) { bot._client.write('name_item', { name }); await h.ticks(3) }
    await bot._syncWindow(a)
    h.result(`${key}.offer`, `cost=${props[0]} result=${comps(a.slots[2])}`)
    await h.click(2, 0, 1)
    await h.ticks(3)
    h.result(`${key}.taken`, `${h.win(0, 3)} level=${bot.experience.level}`)
    await close()
  }
  await anvil('anvil_rename', { 0: 'iron_sword' }, [[0, 0]], 'Blade')
  await anvil('anvil_rename_same', { 0: 'iron_sword[custom_name="Blade"]' }, [[0, 0]], 'Blade')
  await anvil('anvil_book', { 0: 'iron_sword', 1: 'enchanted_book[stored_enchantments={sharpness:3}]' }, [[0, 0], [1, 1]])
  await anvil('anvil_combine', { 0: 'iron_sword[damage=200,enchantments={sharpness:2}]', 1: 'iron_sword[damage=10,enchantments={sharpness:2,unbreaking:1}]' }, [[0, 0], [1, 1]])
  await anvil('anvil_repair_material', { 0: 'iron_sword[damage=200]', 1: 'iron_ingot 5' }, [[0, 0], [1, 1]])
  await anvil('anvil_incompatible', { 0: 'iron_sword', 1: 'enchanted_book[stored_enchantments={efficiency:3}]' }, [[0, 0], [1, 1]])
  await anvil('anvil_too_expensive', { 0: 'iron_sword[repair_cost=39]', 1: 'enchanted_book[stored_enchantments={sharpness:1}]' }, [[0, 0], [1, 1]])
  await anvil('anvil_rename_expensive', { 0: 'iron_sword[repair_cost=39]' }, [[0, 0]], 'Pricey')
  await anvil('anvil_books', { 0: 'enchanted_book[stored_enchantments={sharpness:3}]', 1: 'enchanted_book[stored_enchantments={sharpness:3}]' }, [[0, 0], [1, 1]])

  // Grindstone.
  const grind = async (key, map, slots) => {
    await setLevels(0)
    h.cmd('kill @e[type=experience_orb]')
    const g = await open('grindstone[face=floor]', map)
    for (const [i, s] of slots) await put(inv(g, i), s)
    await bot._syncWindow(g)
    h.result(`${key}.offer`, comps(g.slots[2]))
    await h.click(2, 0, 1)
    await h.ticks(5)
    h.result(`${key}.taken`, `${h.win(0, 3)} orbs=${h.orbs().length > 0}`)
    await close()
  }
  await grind('grind_disenchant', { 0: 'iron_sword[enchantments={sharpness:3},repair_cost=3]' }, [[0, 0]])
  await grind('grind_curse', { 0: 'iron_sword[enchantments={sharpness:3,vanishing_curse:1}]' }, [[0, 0]])
  await grind('grind_book', { 0: 'enchanted_book[stored_enchantments={sharpness:3}]' }, [[0, 1]])
  await grind('grind_combine', { 0: 'iron_sword[damage=200]', 1: 'iron_sword[damage=150]' }, [[0, 0], [1, 1]])

  // Smithing.
  const smith = async (key, map, slots) => {
    const s = await open('smithing_table', map)
    for (const [i, t] of slots) await put(inv(s, i), t)
    await bot._syncWindow(s)
    h.result(`${key}.offer`, comps(s.slots[3]))
    await h.click(3, 0, 1)
    h.result(`${key}.taken`, h.win(0, 4))
    await close()
  }
  await smith('smith_netherite', { 0: 'netherite_upgrade_smithing_template', 1: 'diamond_sword[enchantments={sharpness:3},damage=100]', 2: 'netherite_ingot' }, [[0, 0], [1, 1], [2, 2]])
  await smith('smith_trim', { 0: 'coast_armor_trim_smithing_template 2', 1: 'iron_chestplate', 2: 'redstone 3' }, [[0, 0], [1, 1], [2, 2]])

  // Stonecutter: the result of each recipe button for stone.
  let s = await open('stonecutter', { 0: 'stone 10' })
  await put(inv(s, 0), 0)
  const cut = []
  for (let i = 0; i < 12; i++) { await button(i); cut.push(h.stackStr(s.slots[1])) }
  h.result('stonecutter.buttons', cut.join(','))
  await button(2)
  await h.click(1, 0, 1)
  h.result('stonecutter.shift_take', h.win())
  await close()

  // Loom: buttons pick patterns; the result keeps the banner's colour and adds a layer.
  s = await open('loom', { 0: 'white_banner', 1: 'red_dye 2', 2: 'creeper_banner_pattern' })
  await put(inv(s, 0), 0)
  await put(inv(s, 1), 1)
  const looms = []
  for (const i of [0, 1, 5, 20]) { await button(i); looms.push(comps(s.slots[3])) }
  h.result('loom.buttons', looms.join(' '))
  await put(inv(s, 2), 2)
  await button(0)
  h.result('loom.with_pattern', comps(s.slots[3]))
  await h.click(3, 0, 1)
  h.result('loom.taken', h.win())
  await close()

  // Cartography: copy, lock and zoom a map the bot made by using an empty map.
  await close()
  await h.fill({ 0: 'map 1' })
  bot.setQuickBarSlot(0)
  await h.ticks(2)
  bot.activateItem()
  await h.waitFor(() => h.inv().filled_map, 5000)
  h.result('cart.made_map', h.invStr())
  const cart = async (key, extra) => {
    await place('cartography_table')
    for (const it of ['map', 'paper', 'glass_pane']) h.cmd(`clear ${bot.username} ${it}`)
    h.cmd(`item replace entity ${bot.username} container.20 with ${extra}`)
    await h.waitFor(() => bot.inventory.slots[20], 3000)
    const c = await open()
    const map = c.slots.findIndex((it, i) => i >= 3 && it && it.name === 'filled_map')
    await put(map, 0, 1)
    await put(inv(c, 20), 1)
    await bot._syncWindow(c)
    h.result(`${key}.offer`, comps(c.slots[2]))
    await h.click(2, 0, 1)
    h.result(`${key}.taken`, h.win(0, 3))
    await close()
  }
  await cart('cart_copy', 'map 2')
  await cart('cart_zoom', 'paper 2')
  await cart('cart_lock', 'glass_pane 2')
})
