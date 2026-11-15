// Block entities keep what their item carried: a named container placed by a player shows its
// name as the menu title and drops with it, a shulker box keeps its contents, banners keep
// their patterns (and an ominous banner stays ominous), and pick block with data copies it all.
const { make } = require('./survival-lib')
const h = make('Keeper')
const { bot } = h
const X = 3700; const Y = 200; const Z = 0
const BX = X; const BY = Y + 1; const BZ = Z - 2

const comps = (it) => it ? `${it.name}${JSON.stringify(it.componentMap ? [...it.componentMap.keys()].sort() : [])}` : 'none'
const nameOf = (it) => {
  if (!it) return 'none'
  const name = it.customName
  if (name === undefined || name === null) return 'unnamed'
  return JSON.stringify(name).includes('Treasure') ? 'Treasure' : JSON.stringify(name)
}
// The dropped stack of `item`, on the ground or already picked up.
const drop = (item) => Object.values(bot.entities)
  .filter((e) => e.name === 'item' && e.getDroppedItem())
  .map((e) => e.getDroppedItem())
  .concat(bot.inventory.items())
  .find((it) => it.name === item)
const held = (item) => bot.inventory.items().find((it) => it.name === item)

// Places `spec` from the hand on the floor in front of the bot, like a player would.
const placeItem = async (spec) => {
  const name = spec.split('[')[0]
  h.cmd(`setblock ${BX} ${BY} ${BZ} air`)
  await h.waitFor(() => h.blockName(BX, BY, BZ) === 'air', 3000)
  await h.clear()
  await h.give(spec)
  const it = held(name)
  if (!it) return 'not_given'
  await bot.equip(it, 'hand').catch(() => {})
  await h.ticks(2)
  await bot.placeBlock(h.block(BX, Y, BZ), new h.Vec3(0, 1, 0)).catch(() => {})
  await h.waitFor(() => h.blockName(BX, BY, BZ) !== 'air', 3000)
  await h.ticks(2)
  return h.blockName(BX, BY, BZ)
}
const title = async () => {
  const w = await bot.openBlock(h.block(BX, BY, BZ)).catch(() => null)
  if (!w) return { title: 'refused', items: 'none' }
  await h.ticks(2)
  const text = JSON.stringify(w.title ?? null)
  const items = w.slots.slice(0, w.inventoryStart).filter(Boolean).map(h.stackStr).join(',') || 'empty'
  bot.closeWindow(w)
  await h.ticks(2)
  return { title: text.includes('Treasure') ? 'Treasure' : 'default', items }
}
// Breaks the block with its drops and waits for the dropped `item`.
const breakIt = async (item) => {
  h.cmd('kill @e[type=item]')
  await h.clear()
  h.cmd(`setblock ${BX} ${BY} ${BZ} air destroy`)
  await h.waitFor(() => drop(item), 3000)
  await h.ticks(2)
  return drop(item)
}
const pick = async (item, includeData) => {
  await h.clear()
  bot._client.write('pick_item_from_block', { position: { x: BX, y: BY, z: BZ }, includeData })
  await h.waitFor(() => held(item), 3000)
  return held(item)
}

h.start(async () => {
  await h.arena(X, Y, Z, 6)
  h.cmd(`effect give ${bot.username} resistance infinite 255 true`)
  await h.tp(BX + 0.5, Y + 1, BZ + 2.5, 180, 0)

  // Every container whose loot table copies custom_name.
  for (const block of ['chest', 'trapped_chest', 'barrel', 'furnace', 'smoker', 'blast_furnace', 'hopper',
    'dispenser', 'dropper', 'brewing_stand', 'enchanting_table', 'beacon', 'copper_chest']) {
    const placed = await placeItem(`${block}[custom_name="Treasure"]`)
    if (placed !== block) { h.result(`${block}.placed`, placed); continue }
    h.result(`${block}.title`, (await title()).title)
    const it = await breakIt(block)
    h.result(`${block}.drop`, `${nameOf(it)} ${comps(it)}`)
  }

  // A chest the bot breaks itself goes through the player's loot path.
  h.result('chest.dug_placed', await placeItem('chest[custom_name="Treasure"]'))
  await h.clear()
  h.cmd('kill @e[type=item]')
  await bot.dig(h.block(BX, BY, BZ), true).catch(() => {})
  await h.waitFor(() => drop('chest'), 5000)
  h.result('chest.dug', `${nameOf(drop('chest'))} ${comps(drop('chest'))}`)

  // A name loaded from block entity data.
  h.cmd(`setblock ${BX} ${BY} ${BZ} chest{CustomName:"Treasure"}`)
  await h.waitFor(() => h.blockName(BX, BY, BZ) === 'chest', 3000)
  h.result('chest.loaded_title', (await title()).title)
  h.result('chest.loaded_drop', nameOf(await breakIt('chest')))

  // A shulker box keeps its contents and name.
  h.result('shulker.placed', await placeItem('shulker_box[custom_name="Treasure",container=[{slot:0,item:{id:"diamond",count:3}},{slot:5,item:{id:"stick"}}]]'))
  const box = await title()
  h.result('shulker.opened', `${box.title} ${box.items}`)
  const boxDrop = await breakIt('shulker_box')
  h.result('shulker.drop', `${nameOf(boxDrop)} ${comps(boxDrop)}`)

  // A banner keeps its patterns; an ominous banner keeps its item name, rarity and tooltip.
  h.result('banner.placed', await placeItem('red_banner[custom_name="Treasure",banner_patterns=[{pattern:"minecraft:creeper",color:"black"}]]'))
  const banner = await breakIt('red_banner')
  h.result('banner.drop', `${nameOf(banner)} ${comps(banner)}`)
  h.result('ominous_banner.placed', await placeItem('white_banner[item_name={translate:"block.minecraft.ominous_banner"},rarity="uncommon",' +
    'tooltip_display={hidden_components:["banner_patterns"]},banner_patterns=[{pattern:"minecraft:rhombus",color:"cyan"}],lore=["x"]]'))
  h.result('ominous_banner.drop', comps(await breakIt('white_banner')))

  // Lore isn't in the chest's loot table: it stays on the block but not on the drop.
  h.result('chest.lore_placed', await placeItem('chest[custom_name="Treasure",lore=["x"]]'))
  h.result('chest.lore_drop', comps(await breakIt('chest')))

  // Pick block (middle click) in creative, with data (ctrl) and without.
  h.result('pick.placed', await placeItem('chest[custom_name="Treasure",lore=["x"]]'))
  h.cmd(`item replace block ${BX} ${BY} ${BZ} container.3 with diamond 2`)
  h.cmd(`gamemode creative ${bot.username}`)
  await h.waitFor(() => bot.game.gameMode === 'creative', 5000)
  const picked = await pick('chest', true)
  h.result('pick.data', `${nameOf(picked)} ${comps(picked)}`)
  h.result('pick.plain', comps(await pick('chest', false)))
  h.cmd(`setblock ${BX} ${BY} ${BZ} red_banner{patterns:[{pattern:"minecraft:creeper",color:"black"}]}`)
  await h.waitFor(() => h.blockName(BX, BY, BZ) === 'red_banner', 3000)
  h.result('pick.banner', comps(await pick('red_banner', false)))

  // A creative player breaking a filled shulker box gets it with every component, lore too; an
  // empty one drops nothing.
  h.result('shulker.creative_placed', await placeItem('shulker_box[custom_name="Treasure",lore=["x"],' +
    'container=[{slot:0,item:{id:"diamond",count:3}}]]'))
  h.cmd('kill @e[type=item]')
  await h.clear()
  await bot.dig(h.block(BX, BY, BZ), true).catch(() => {})
  await h.waitFor(() => drop('shulker_box'), 3000)
  const creativeBox = drop('shulker_box')
  h.result('shulker.creative_drop', `${nameOf(creativeBox)} ${comps(creativeBox)}`)
  h.result('shulker.creative_empty_placed', await placeItem('shulker_box[lore=["x"]]'))
  h.cmd('kill @e[type=item]')
  await h.clear()
  await bot.dig(h.block(BX, BY, BZ), true).catch(() => {})
  await h.serverTicks(20)
  h.result('shulker.creative_empty_drop', comps(drop('shulker_box')))
  h.cmd(`setblock ${BX} ${BY} ${BZ} air`)
  h.cmd(`gamemode survival ${bot.username}`)
  await h.ticks(5)
})
