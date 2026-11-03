// Chance and limits around workstations and creative inventories: anvils wearing down through
// chipped and damaged to gone (never in creative), grindstone XP between half and all of the
// enchantments' minimum costs, creative slot packets (slot range, over-stacked items, drops and
// the drop spam limit, ignored outside creative), and a villager restocking at its job site.
const { make } = require('./survival-lib')
const h = make('Wearer')
const { bot } = h
const X = 3300; const Y = 200; const Z = 0
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
const close = async () => { if (bot.currentWindow) bot.closeWindow(bot.currentWindow); await h.ticks(5) }
const invSlot = (w, i) => w.slots.length - 36 + (i < 9 ? 27 + i : i - 9)
const setLevels = async (n) => {
  h.cmd(`xp set ${bot.username} 0 points`)
  h.cmd(`xp set ${bot.username} ${n} levels`)
  await h.waitFor(() => bot.experience.level === n, 3000)
}
const droppedCount = (name) => Object.values(bot.entities)
  .filter((e) => e.name === 'item' && e.getDroppedItem?.()?.name === name)
  .reduce((sum, e) => sum + e.getDroppedItem().count, 0)
const creativeSlot = (slot, name, count) => {
  const Item = require('prismarine-item')(bot.registry)
  bot._client.write('set_creative_slot', { slot, item: Item.toNotch(name ? new Item(bot.registry.itemsByName[name].id, count) : null) })
}

// Renames a fresh paper in the open anvil and takes it; returns the anvil block afterwards.
const renameOnce = async (w, i) => {
  h.cmd(`clear ${bot.username} paper`)
  h.cmd(`item replace entity ${bot.username} container.0 with paper`)
  await h.waitFor(() => w.slots[invSlot(w, 0)], 3000)
  await h.click(invSlot(w, 0), 0, 0)
  await h.click(0, 0, 0)
  bot._client.write('name_item', { name: `n${i}` })
  await h.ticks(2)
  await h.click(2, 0, 1)
  await h.ticks(2)
  return h.blockName(BX, BY, BZ)
}
// Uses an anvil until it breaks (or `limit` uses) and lists the states it went through.
const wear = async (limit) => {
  await place('anvil[facing=north]')
  let w = await open()
  const states = ['anvil']
  for (let i = 0; i < limit; i++) {
    const state = await renameOnce(w, i)
    if (state !== states[states.length - 1]) states.push(state)
    if (state === 'air') break
    if (!bot.currentWindow) w = await open()
  }
  await close()
  return states.join('>')
}

h.start(async () => {
  await h.arena(X, Y, Z, 5)

  // Anvils: creative uses never damage it; survival uses go through every state.
  h.cmd(`gamemode creative ${bot.username}`)
  await h.waitFor(() => bot.game.gameMode === 'creative', 5000)
  h.result('anvil.creative_40_uses', await wear(40))
  h.cmd(`gamemode survival ${bot.username}`)
  await h.waitFor(() => bot.game.gameMode === 'survival', 5000)
  await setLevels(1000)
  h.result('anvil.survival', await wear(300))

  // Grindstone: sharpness V (minimum cost 45) gives 23 to 45 XP.
  await place('grindstone[face=floor]')
  const xp = []
  for (let i = 0; i < 6; i++) {
    await h.fill({ 0: 'iron_sword[enchantments={sharpness:5}]' })
    const before = bot.experience.points
    const w = await open()
    await h.click(invSlot(w, 0), 0, 0)
    await h.click(0, 0, 0)
    await h.click(2, 0, 1)
    await close()
    await h.ticks(60)
    xp.push(bot.experience.points - before)
  }
  h.result('grindstone.sharpness5_in_23_45', xp.every((p) => p >= 23 && p <= 45) ? 'yes' : xp.join(','))

  // Creative slot packets.
  await h.clear()
  h.cmd(`gamemode creative ${bot.username}`)
  await h.waitFor(() => bot.game.gameMode === 'creative', 5000)
  creativeSlot(36, 'diamond', 5)
  creativeSlot(37, 'diamond', 99)
  creativeSlot(46, 'emerald', 3)
  creativeSlot(0, 'gold_ingot', 2)
  await h.ticks(10)
  // Accepted slots aren't echoed back; a stale click makes the server send the whole inventory.
  await h.click(9)
  h.result('creative.slots', h.slots())
  h.cmd(`kill @e[type=item]`)
  await h.ticks(5)
  creativeSlot(-1, 'stick', 3)
  creativeSlot(-1, 'stick', 99)
  await h.ticks(10)
  h.result('creative.drop', droppedCount('stick'))
  h.cmd(`kill @e[type=item]`)
  await h.ticks(5)
  for (let i = 0; i < 90; i++) creativeSlot(-1, 'dirt', 1)
  await h.ticks(20)
  h.result('creative.drop_spam', droppedCount('dirt'))
  h.cmd(`kill @e[type=item]`)
  // Outside creative the packet is ignored.
  h.cmd(`gamemode survival ${bot.username}`)
  await h.waitFor(() => bot.game.gameMode === 'survival', 5000)
  await h.clear()
  creativeSlot(36, 'diamond', 5)
  creativeSlot(-1, 'diamond', 5)
  await h.ticks(10)
  await h.click(9)
  h.result('creative.survival_ignored', `${h.slots()} dropped=${droppedCount('diamond')}`)

  // A librarian with used-up trades restocks at its lectern during the work day.
  h.cmd('kill @e[type=villager]')
  await place('lectern[facing=north]', X - 2, Y + 1, Z + 2)
  h.cmd(`summon villager ${X - 2.5} ${Y + 1} ${Z + 3.5} {VillagerData:{profession:"minecraft:librarian",level:1,type:"minecraft:plains"},Xp:1,Offers:{Recipes:[{buy:{id:"minecraft:paper",count:24},sell:{id:"minecraft:emerald",count:1},maxUses:16,uses:16,xp:2,priceMultiplier:0.05f}]}}`)
  h.cmd('time set 2500')
  const lists = []
  bot._client.on('trade_list', (p) => lists.push(p))
  await h.tp(X - 2.5, Y + 1, Z - 1.5)
  await h.ticks(1200)
  const villager = Object.values(bot.entities).find((e) => e.name === 'villager')
  if (villager) {
    const w = await bot.openVillager(villager)
    await h.waitFor(() => lists.length > 0, 5000)
    const t = lists[lists.length - 1].trades[0]
    h.result('villager.restock', t ? `uses=${t.nbTradeUses}/${t.maximumNbTradeUses} demand=${t.demand}` : 'no trades')
    w.close()
  } else {
    h.result('villager.restock', 'no villager')
  }
})
