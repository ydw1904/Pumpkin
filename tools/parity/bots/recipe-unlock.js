// Recipe book unlocking: a new player only knows what recipe advancements gave them, picking up
// items unlocks more, /recipe give and take answer with the displays they add or remove,
// crafting a recipe learns it, and limited_crafting keeps unknown recipes out of the grid.
// Display ids differ between servers, so recipes are named by their result.
const { make } = require('./survival-lib')
const h = make('Unlocker')
const { bot } = h
const X = 3300; const Y = 200; const Z = 0

const known = new Map()
const resultName = (r) => {
  const d = r.display.data.result
  if (!d) return '?'
  const id = d.type === 'item_stack' ? d.data.itemId : d.data
  return bot.registry.items[id]?.name ?? d.type
}
// Each packet as `replace|added results with flags`, sorted, so the order of entries doesn't matter.
const adds = []
bot._client.on('recipe_book_add', (p) => {
  if (p.replace) known.clear()
  const shown = p.entries.map((e) => {
    known.set(e.recipe.displayId, e.recipe)
    return `${resultName(e.recipe)}:${JSON.stringify(e.flags)}`
  }).sort()
  adds.push(`${p.replace ? 'replace' : 'add'}[${shown.join(',')}]`)
})
const removes = []
bot._client.on('recipe_book_remove', (p) => {
  removes.push(p.recipeIds.length)
  for (const id of p.recipeIds) known.delete(id)
})
const events = []
bot._client.on('packet', (data, meta) => {
  if (meta.name === 'game_state_change' && data.reason === 'limited_crafting') events.push(`limited=${data.gameMode}`)
})
const chat = []
bot.on('messagestr', (m) => chat.push(m))

const knownResults = () => [...new Set([...known.values()].map(resultName))].sort().join(',') || 'none'
// Packets sorted too: servers walk the recipe advancements in different orders.
const takeAdds = () => { const out = adds.sort().join(' '); adds.length = 0; return out || 'none' }
const say = async (command) => {
  chat.length = 0
  bot.chat(command)
  await h.waitFor(() => chat.length > 0, 3000)
  await h.ticks(5)
  return chat.join(' | ')
}
// Puts one oak plank into slots 1 and 3 of the inventory's 2x2 grid (result slot 0).
const fillGrid = async () => {
  for (const slot of [1, 2, 3, 4]) if (bot.inventory.slots[slot]) await h.click(slot, 0, 1)
  await h.fill({ 9: 'oak_planks 2' })
  await h.click(9, 0, 0)
  await h.click(1, 1, 0)
  await h.click(3, 1, 0)
}
const gridResult = async () => {
  await h.ticks(5)
  const r = bot.inventory.slots[0]
  return r ? `${r.name}*${r.count}` : 'empty'
}

h.start(async () => {
  await h.arena(X, Y, Z, 3)
  await h.ticks(20)
  // A new player gets what `unlock_right_away` advancements give.
  h.result('join.known', knownResults())

  // Holding an oak log completes the oak plank and wood recipe advancements.
  takeAdds()
  await h.fill({ 0: 'oak_log 1' })
  await h.ticks(20)
  h.result('pickup.adds', takeAdds())

  h.result('give.feedback', await say('/recipe give @s minecraft:stick'))
  h.result('give.adds', takeAdds())
  h.result('give_again.feedback', await say('/recipe give @s minecraft:stick'))
  h.result('give_special.feedback', await say('/recipe give @s minecraft:firework_rocket'))
  h.result('take.feedback', await say('/recipe take @s minecraft:stick'))
  h.result('take.removed', removes.join(','))
  h.result('take_again.feedback', await say('/recipe take @s minecraft:stick'))

  // Crafting sticks in the inventory grid learns the recipe.
  await fillGrid()
  h.result('craft.grid', await gridResult())
  takeAdds()
  await h.click(0, 0, 1)
  await h.ticks(10)
  h.result('craft.adds', takeAdds())

  // With limited_crafting, an unknown recipe leaves the result empty until it is learned.
  h.cmd('gamerule limited_crafting true')
  await h.waitFor(() => events.length > 0, 3000)
  h.result('limited.event', events.join(','))
  await say('/recipe take @s minecraft:stick')
  await fillGrid()
  h.result('limited.unknown_grid', await gridResult())
  await say('/recipe give @s minecraft:stick')
  await fillGrid()
  h.result('limited.known_grid', await gridResult())
  h.cmd('gamerule limited_crafting false')
  await h.waitFor(() => events.length > 1, 3000)
  h.result('limited.events', events.join(','))
  await h.clear()
  h.result('end.known', knownResults())
})
