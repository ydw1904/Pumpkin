// Copper golems: building one from a copper block and a carved pumpkin, sorting items from a
// copper chest into a matching chest, and copper golem statues (placement facing, pose changes,
// reviving with an axe). Records the blocks and entity data the client sees.
const { make } = require('./survival-lib')
const h = make('Sorter')
const { bot } = h
const X = 11200; const Y = 200; const Z = 0
const WEATHER = 16
const STATE = 17

const golems = () => Object.values(bot.entities).filter(e => e.name === 'copper_golem')
const props = (x, y, z) => {
  const b = h.block(x, y, z)
  if (!b) return 'unloaded'
  const p = b.getProperties()
  return `${b.name}[${Object.keys(p).sort().filter(k => k !== 'waterlogged').map(k => `${k}=${p[k]}`).join(',')}]`
}
const frac = (v) => (v - Math.floor(v)).toFixed(2)

async function hold (item) {
  await h.fill({ 0: item })
  bot.setQuickBarSlot(0)
  await h.ticks(5)
}

async function contents (x, y, z) {
  const window = await bot.openBlock(h.block(x, y, z))
  const items = window.containerItems().map(it => `${it.name}*${it.count}`).sort().join(',') || 'empty'
  window.close()
  await h.ticks(5)
  return items
}

h.start(async () => {
  await h.arena(X, Y, Z, 8)
  h.cmd('time set noon')
  h.cmd('gamerule advance_time false')
  h.cmd(`tp ${bot.username} ${X + 0.5} ${Y + 1} ${Z + 0.5} 0 30`)
  await h.ticks(20)

  // Build: the pumpkin goes on an exposed copper block two blocks south of the bot.
  h.cmd(`setblock ${X} ${Y + 1} ${Z + 2} exposed_copper`)
  await h.waitFor(() => h.blockName(X, Y + 1, Z + 2) === 'exposed_copper', 5000)
  await hold('carved_pumpkin')
  await bot.placeBlock(h.block(X, Y + 1, Z + 2), new h.Vec3(0, 1, 0)).catch(() => {})
  let golem
  await h.waitFor(() => (golem = golems()[0]), 5000)
  await h.ticks(10)
  h.result('build.golem', !!golem)
  if (golem) {
    const p = golem.position
    h.result('build.golem_offset', `${frac(p.x)},${(p.y - (Y + 1)).toFixed(2)},${frac(p.z)}`)
    h.result('build.weather_state', golem.metadata[WEATHER])
    h.result('build.state', golem.metadata[STATE] ?? 0)
  }
  h.result('build.copper_block', props(X, Y + 1, Z + 2))
  h.result('build.pumpkin_block', props(X, Y + 2, Z + 2))
  h.cmd('kill @e[type=copper_golem]')
  h.cmd(`setblock ${X} ${Y + 1} ${Z + 2} air`)
  await h.ticks(10)

  // Sorting: 20 iron ingots in a copper chest, one in a chest; the golem carries 16 at a time.
  h.cmd(`setblock ${X - 3} ${Y + 1} ${Z - 4} copper_chest[facing=south]{Items:[{Slot:0b,id:"minecraft:iron_ingot",count:20}]}`)
  h.cmd(`setblock ${X + 3} ${Y + 1} ${Z - 4} chest[facing=south]{Items:[{Slot:0b,id:"minecraft:iron_ingot",count:1}]}`)
  h.cmd(`setblock ${X - 1} ${Y + 1} ${Z - 4} chest[facing=south]{Items:[{Slot:0b,id:"minecraft:stone",count:1}]}`)
  await h.waitFor(() => h.blockName(X + 3, Y + 1, Z - 4) === 'chest', 5000)
  h.cmd(`summon copper_golem ${X + 0.5} ${Y + 1} ${Z - 1.5}`)
  await h.waitFor(() => (golem = golems()[0]), 5000)
  const states = []
  const sample = () => {
    const s = golem.metadata[STATE] ?? 0
    if (s !== states[states.length - 1]) states.push(s)
  }
  bot.on('physicsTick', sample)
  await h.serverTicks(1200)
  bot.removeListener('physicsTick', sample)
  h.result('sort.states_first_8', states.slice(0, 8).join('>'))
  h.result('sort.held', golem.equipment && golem.equipment[0] ? `${golem.equipment[0].name}*${golem.equipment[0].count}` : 'empty')
  h.cmd('kill @e[type=copper_golem]')
  h.cmd('kill @e[type=item]')
  await h.ticks(10)
  h.result('sort.copper_chest', await contents(X - 3, Y + 1, Z - 4))
  h.result('sort.matching_chest', await contents(X + 3, Y + 1, Z - 4))
  h.result('sort.other_chest', await contents(X - 1, Y + 1, Z - 4))

  // Statue: placed facing the player, a click changes the pose, an axe brings the golem back.
  h.cmd(`tp ${bot.username} ${X + 0.5} ${Y + 1} ${Z + 0.5} 0 30`)
  await h.ticks(10)
  await hold('copper_golem_statue')
  await bot.placeBlock(h.block(X, Y, Z + 2), new h.Vec3(0, 1, 0)).catch(() => {})
  await h.ticks(10)
  h.result('statue.placed', props(X, Y + 1, Z + 2))
  await hold('stone 1')
  await bot.activateBlock(h.block(X, Y + 1, Z + 2)).catch(() => {})
  await h.ticks(10)
  h.result('statue.after_click', props(X, Y + 1, Z + 2))
  await hold('iron_axe')
  await bot.activateBlock(h.block(X, Y + 1, Z + 2)).catch(() => {})
  await h.waitFor(() => golems().length > 0, 3000)
  await h.ticks(5)
  h.result('statue.after_axe', props(X, Y + 1, Z + 2))
  h.result('statue.golem', golems().length)
  const axe = bot.inventory.slots[36]
  h.result('statue.axe_damage', axe ? (axe.durabilityUsed ?? 'unknown') : 'gone')
  h.cmd('kill @e[type=!player]')
})
