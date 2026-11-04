// Containers with a loot table fill themselves the first time anything reads them: a player
// opening a barrel, a hopper pulling from a chest, a dispenser broken for its drops, a decorated
// pot broken, and a chest minecart opened.
// Loot rolls differ between servers, so only whether something came out is compared.
const { make } = require('./survival-lib')
const h = make('Looter')
const { bot } = h
const X = 3600; const Y = 200; const Z = 0
const BX = X; const BY = Y + 1; const BZ = Z - 2
const TABLE = 'minecraft:chests/simple_dungeon'

const place = async (block, x = BX, y = BY, z = BZ) => {
  const name = block.split('[')[0].split('{')[0]
  for (let i = 0; i < 5; i++) {
    h.cmd(`setblock ${x} ${y} ${z} air`)
    h.cmd(`setblock ${x} ${y} ${z} ${block}`)
    if (await h.waitFor(() => h.blockName(x, y, z) === name, 2000)) break
  }
  await h.ticks(2)
}
const filled = (window, end) => window.slots.slice(0, end).some(Boolean) ? 'filled' : 'empty'
const openBlock = async (x = BX, y = BY, z = BZ) => {
  const w = await bot.openBlock(h.block(x, y, z)).catch(() => null)
  if (!w) return 'refused'
  await h.ticks(2)
  const result = filled(w, w.inventoryStart)
  bot.closeWindow(w)
  await h.ticks(2)
  return result
}
// Dropped items and whatever the bot already picked up.
const dropped = (except) => [
  ...Object.values(bot.entities).filter((e) => e.name === 'item').map((e) => e.getDroppedItem()),
  ...bot.inventory.items()
].some((it) => it && !except.includes(it.name)) ? 'drops' : 'nothing'
const nearest = (name) => Object.values(bot.entities)
  .filter((e) => e.name === name)
  .sort((a, b) => a.position.distanceTo(bot.entity.position) - b.position.distanceTo(bot.entity.position))[0]

h.start(async () => {
  await h.arena(X, Y, Z, 6)
  h.cmd(`effect give ${bot.username} resistance infinite 255 true`)
  await h.tp(BX + 0.5, Y + 1, BZ + 2.5, 180, 0)

  await place(`barrel[facing=up]{LootTable:"${TABLE}"}`)
  h.result('barrel.opened', await openBlock())

  // A hopper under the chest pulls the unpacked loot.
  await place('hopper[facing=down]', BX, BY, BZ)
  await place(`chest{LootTable:"${TABLE}"}`, BX, BY + 1, BZ)
  await h.ticks(20)
  h.result('chest.hopper_pulled', await openBlock(BX, BY, BZ))
  await place('air', BX, BY + 1, BZ)

  // Breaking a dispenser drops its unpacked loot.
  h.cmd('kill @e[type=item]')
  await h.clear()
  await place(`dispenser[facing=up]{LootTable:"${TABLE}"}`)
  h.cmd(`setblock ${BX} ${BY} ${BZ} air destroy`)
  await h.ticks(10)
  h.result('dispenser.broken', dropped(['dispenser']))

  // A decorated pot keeps its own loot table.
  h.cmd('kill @e[type=item]')
  await h.clear()
  await place('decorated_pot{LootTable:"minecraft:pots/trial_chambers/corridor"}')
  h.cmd(`setblock ${BX} ${BY} ${BZ} air destroy`)
  await h.ticks(10)
  h.result('pot.broken', dropped(['decorated_pot', 'brick']))
  h.cmd('kill @e[type=item]')

  // Spectators can't open a container whose loot is still pending.
  await place(`barrel[facing=up]{LootTable:"${TABLE}"}`)
  h.cmd(`gamemode spectator ${bot.username}`)
  await h.waitFor(() => bot.game.gameMode === 'spectator', 5000)
  h.result('barrel.spectator', await openBlock())
  h.cmd(`gamemode survival ${bot.username}`)
  await h.waitFor(() => bot.game.gameMode === 'survival', 5000)
  await place('air')

  h.cmd(`summon chest_minecart ${BX + 0.5} ${BY} ${BZ + 0.5} {LootTable:"${TABLE}"}`)
  await h.waitFor(() => nearest('chest_minecart'), 5000)
  await h.ticks(5)
  const cart = nearest('chest_minecart')
  if (cart) {
    const opened = new Promise((resolve) => { bot.once('windowOpen', resolve); setTimeout(() => resolve(null), 3000) })
    await bot.activateEntity(cart).catch(() => {})
    const w = await opened
    await h.ticks(2)
    h.result('chest_minecart.opened', w ? filled(w, w.inventoryStart) : 'refused')
    if (w) bot.closeWindow(w)
    h.cmd('kill @e[type=chest_minecart]')
  } else h.result('chest_minecart.opened', 'missing')
  await h.ticks(5)
})
