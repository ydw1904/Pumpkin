// Tool durability per action: mining with the right tool, the wrong tool and on blocks that break
// instantly, tilling, lighting fire, and Mending paying for repairs with experience orbs.
const { make } = require('./survival-lib')
const h = make('Wearer')
const { bot } = h
const X = 2100; const Y = 200; const Z = 0

async function hold (item) {
  h.cmd(`clear ${bot.username}`)
  h.cmd(`item replace entity ${bot.username} hotbar.0 with ${item}`)
  bot.setQuickBarSlot(0)
  const name = item.split('[')[0]
  await h.waitFor(() => bot.heldItem && bot.heldItem.name === name, 5000)
  await h.ticks(5)
}
const used = () => bot.heldItem ? bot.heldItem.durabilityUsed : 'gone'

async function mine (item, block) {
  await hold(item)
  h.cmd(`setblock ${X + 1} ${Y + 1} ${Z} ${block}`)
  await h.waitFor(() => h.blockName(X + 1, Y + 1, Z) === block, 3000)
  await h.ticks(5)
  try {
    await bot.dig(h.block(X + 1, Y + 1, Z), true)
  } catch (e) {
    return `error ${e.message}`
  }
  await h.ticks(5)
  return `${h.blockName(X + 1, Y + 1, Z)} used=${used()}`
}

h.start(async () => {
  await h.arena(X, Y, Z, 4)
  h.cmd('difficulty peaceful')
  h.cmd(`tp ${bot.username} ${X + 0.5} ${Y + 1} ${Z + 0.5} -90 0`)
  await h.ticks(10)

  for (const [item, block] of [
    ['iron_pickaxe', 'stone'], ['iron_pickaxe', 'dirt'], ['iron_sword', 'dirt'], ['iron_sword', 'cobweb'],
    ['iron_sword', 'short_grass'], ['iron_axe', 'oak_planks'], ['iron_shovel', 'torch'], ['shears', 'white_wool'],
    ['shears', 'oak_leaves'], ['iron_hoe', 'hay_block'], ['iron_pickaxe[enchantments={silk_touch:1}]', 'glass'],
  ]) {
    h.result(`mine.${item}.${block}`, await mine(item, block))
  }

  // Tilling dirt and lighting a fire each cost one point.
  await hold('iron_hoe')
  h.cmd(`setblock ${X + 1} ${Y + 1} ${Z} air`)
  h.cmd(`setblock ${X + 1} ${Y} ${Z} dirt`)
  await h.waitFor(() => h.blockName(X + 1, Y, Z) === 'dirt', 3000)
  await bot.activateBlock(h.block(X + 1, Y, Z), new h.Vec3(0, 1, 0))
  await h.ticks(5)
  h.result('till.iron_hoe', `${h.blockName(X + 1, Y, Z)} used=${used()}`)
  await hold('flint_and_steel')
  h.cmd(`setblock ${X + 1} ${Y} ${Z} stone`)
  await h.ticks(5)
  await bot.activateBlock(h.block(X + 1, Y, Z), new h.Vec3(0, 1, 0))
  await h.ticks(5)
  h.result('ignite.flint_and_steel', `${h.blockName(X + 1, Y + 1, Z)} used=${used()}`)
  h.cmd(`setblock ${X + 1} ${Y + 1} ${Z} air`)

  // Mending: an orb repairs two points per experience point and the rest goes to the player.
  for (const [damage, value] of [[100, 10], [10, 50]]) {
    h.cmd(`xp set ${bot.username} 0 points`)
    h.cmd(`xp set ${bot.username} 0 levels`)
    await hold(`iron_sword[damage=${damage},enchantments={mending:1}]`)
    await h.waitFor(() => bot.experience.points === 0, 3000)
    h.cmd(`summon experience_orb ${X + 2.5} ${Y + 1} ${Z + 0.5} {Value:${value}}`)
    await h.waitFor(() => used() !== damage, 8000)
    await h.ticks(20)
    h.result(`mending.damage${damage}.orb${value}`, `used=${used()} xp=${bot.experience.level}/${Math.round(bot.experience.progress * 100)}`)
  }
})
