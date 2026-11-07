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
})
