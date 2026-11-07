// The Used stat for items used on blocks: only an item use that does something counts, and
// blocks that take the item count it themselves.
const { make } = require('./survival-lib')
const h = make('UsedStat')
const { bot, Vec3 } = h
const X = 7600; const Y = 200; const Z = 0
// Vanilla `BuiltInRegistries.STAT_TYPE` order: mined, crafted, used, ...
const USED = 2

async function usedStats () {
  const stats = new Promise((resolve) => {
    bot._client.once('statistics', (p) => resolve(p.entries))
    setTimeout(() => resolve([]), 5000)
  })
  bot._client.write('client_command', { actionId: 'request_stats' })
  const used = {}
  for (const e of await stats) if (e.categoryId === USED) used[bot.registry.items[e.statisticId].name] = e.value
  return used
}
async function hold (name) {
  await h.give(name, 4)
  await bot.equip(bot.inventory.items().find(i => i.name === name), 'hand')
}
async function click (x, y, z) {
  await bot.lookAt(new Vec3(x + 0.5, y + 1, z + 0.5), true)
  await bot.activateBlock(h.block(x, y, z), new Vec3(0, 1, 0)).catch(e => h.result('use_error', e.message))
  await h.ticks(10)
}

h.start(async () => {
  await h.arena(X, Y, Z, 6)
  h.cmd(`gamemode survival ${bot.username}`)
  await h.ticks(10)
  const before = await usedStats()
  const delta = async (key, name) => {
    const now = await usedStats()
    // Vanilla only sends the stats that changed since the last request.
    const value = now[name] ?? before[name] ?? 0
    h.result(key, value - (before[name] || 0))
    before[name] = value
  }

  // A stick does nothing on stone.
  await hold('stick')
  await click(X + 2, Y, Z)
  await delta('stick_on_stone', 'stick')

  // A placed block counts; one that can't be placed doesn't.
  await hold('stone')
  await click(X + 2, Y, Z + 2)
  await delta('stone_placed', 'stone')
  h.cmd(`setblock ${X + 2} ${Y + 1} ${Z - 2} barrier`)
  h.cmd(`setblock ${X + 2} ${Y + 2} ${Z - 2} barrier`)
  await h.ticks(5)
  await click(X + 2, Y + 1, Z - 2)
  await delta('stone_blocked', 'stone')

  // Opening a chest with an item in hand doesn't use the item.
  h.cmd(`setblock ${X - 2} ${Y + 1} ${Z} chest`)
  await h.ticks(5)
  await hold('stick')
  await click(X - 2, Y + 1, Z)
  bot.closeWindow(bot.currentWindow || bot.inventory)
  await h.ticks(5)
  await delta('stick_on_chest', 'stick')

  // Blocks that take the item count it once.
  h.cmd(`setblock ${X - 2} ${Y + 1} ${Z + 2} composter`)
  await h.ticks(5)
  await hold('wheat_seeds')
  await click(X - 2, Y + 1, Z + 2)
  await delta('seeds_in_composter', 'wheat_seeds')
  h.cmd(`setblock ${X - 2} ${Y + 1} ${Z - 2} cauldron`)
  await h.ticks(5)
  await hold('water_bucket')
  await click(X - 2, Y + 1, Z - 2)
  await delta('bucket_in_cauldron', 'water_bucket')
  h.cmd(`setblock ${X} ${Y + 1} ${Z + 4} tnt`)
  await h.ticks(5)
  await hold('flint_and_steel')
  await click(X, Y + 1, Z + 4)
  await delta('flint_on_tnt', 'flint_and_steel')
  h.cmd('kill @e[type=tnt]')

  // An item use on a block that works.
  await click(X + 4, Y, Z)
  await delta('flint_on_stone', 'flint_and_steel')
  h.cmd(`fill ${X - 6} ${Y + 1} ${Z - 6} ${X + 6} ${Y + 3} ${Z + 6} air`)
  await h.ticks(5)

  // Uses in the air: only items whose own use counts it.
  await bot.look(0, 0.3, true)
  const air = async (key, name, holdTicks = 0) => {
    await hold(name)
    bot.activateItem()
    if (holdTicks) await h.ticks(holdTicks)
    bot.deactivateItem()
    await h.ticks(10)
    await delta(key, name)
  }
  await air('stick_in_air', 'stick')
  await air('snowball_in_air', 'snowball')
  // Consumables count once they finish.
  await air('milk_started', 'milk_bucket', 5)
  await air('milk_finished', 'milk_bucket', 40)
  // An equippable swaps into its slot, and a second use with the same item swaps nothing.
  await h.clear()
  await air('helmet_equipped', 'iron_helmet')
  h.cmd(`item replace entity ${bot.username} armor.head with iron_helmet`)
  await h.ticks(5)
  await air('helmet_same', 'iron_helmet')
  // Casting a fishing rod counts, reeling in doesn't.
  await air('rod_cast', 'fishing_rod')
  await air('rod_reel', 'fishing_rod')
  // A goat horn counts when played and is still there once it has finished.
  await h.clear()
  await air('horn_played', 'goat_horn')
  await h.ticks(160)
  h.result('horn_after_playing', h.inv().goat_horn || 0)

  // Mining counts only items with a tool component, and only outside creative.
  await h.clear()
  const dig = async (key, name, x, z) => {
    h.cmd(`setblock ${x} ${Y + 1} ${z} dirt`)
    await h.waitFor(() => h.blockName(x, Y + 1, z) === 'dirt', 3000)
    await hold(name)
    await bot.lookAt(new Vec3(x + 0.5, Y + 1.5, z + 0.5), true)
    await bot.dig(h.block(x, Y + 1, z), true).catch(e => h.result(`${key}_error`, e.message))
    await h.ticks(10)
    await delta(key, name)
  }
  await dig('stick_mines_dirt', 'stick', X + 2, Z)
  await dig('shovel_mines_dirt', 'iron_shovel', X - 2, Z)

  // Hits count only weapons on living targets; using an item on a mob doesn't count.
  const hit = async (key, name) => {
    h.cmd('kill @e[type=pig]')
    h.cmd(`summon pig ${X + 1.5} ${Y + 1} ${Z + 0.5} {NoAI:1b}`)
    await h.waitFor(() => Object.values(bot.entities).some(e => e.name === 'pig'), 3000)
    await hold(name)
    await h.ticks(30)
    const pig = Object.values(bot.entities).find(e => e.name === 'pig')
    if (pig) bot.attack(pig)
    await h.ticks(10)
    await delta(key, name)
  }
  await hit('stick_hits_pig', 'stick')
  await hit('sword_hits_pig', 'iron_sword')
  h.cmd('kill @e[type=pig]')
  h.cmd(`summon sheep ${X + 1.5} ${Y + 1} ${Z + 0.5} {NoAI:1b}`)
  await h.waitFor(() => Object.values(bot.entities).some(e => e.name === 'sheep'), 3000)
  await hold('shears')
  const sheep = Object.values(bot.entities).find(e => e.name === 'sheep')
  if (sheep) bot.activateEntity(sheep)
  await h.ticks(10)
  await delta('shears_on_sheep', 'shears')
  h.cmd('kill @e[type=sheep]')
  h.cmd('kill @e[type=item]')
})
