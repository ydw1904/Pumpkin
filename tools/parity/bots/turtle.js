// Turtles: a turtle with an egg near home starts digging into sand, two turtles in love breed into
// an egg instead of a baby, and a turtle carrying an egg won't take seagrass.
const { make } = require('./survival-lib')
const h = make('Keeper')
const { bot } = h
const X = 9400; const Y = 200; const Z = 0; const R = 4
// Turtle entity data: 18 is has_egg, 19 laying_egg.
const HAS_EGG = 18; const LAYING_EGG = 19

const turtles = () => Object.values(bot.entities).filter((e) => e.name === 'turtle')

function eggBlocks () {
  let n = 0
  for (let x = X - R; x <= X + R; x++) {
    for (let z = Z - R; z <= Z + R; z++) {
      if (h.blockName(x, Y + 1, z) === 'turtle_egg') n++
    }
  }
  return n
}

// Only the start of the digging is compared: TurtleGoHomeGoal restarts on every full goal tick
// while the turtle has an egg and re-paths it off the sand, so when the egg comes depends on rolls.
function state (label) {
  const all = turtles()
  const eggs = all.filter((t) => t.metadata[HAS_EGG]).length
  const laying = all.filter((t) => t.metadata[LAYING_EGG]).length
  h.result(label, `turtles=${all.length} has_egg=${eggs} laying=${laying} egg_blocks=${eggBlocks()}`)
}

async function reset () {
  h.cmd(`fill ${X - R} ${Y} ${Z - R} ${X + R} ${Y} ${Z + R} sand`)
  h.cmd('kill @e[type=turtle]')
  h.cmd('kill @e[type=experience_orb]')
  h.cmd(`fill ${X - R} ${Y + 1} ${Z - R} ${X + R} ${Y + 2} ${Z + R} air`)
  await h.waitFor(() => turtles().length === 0, 5000)
  await h.ticks(5)
}

h.start(async () => {
  await h.arena(X, Y, Z, 6)
  // Stone under the sand so it doesn't fall.
  h.cmd(`fill ${X - R} ${Y - 1} ${Z - R} ${X + R} ${Y - 1} ${Z + R} stone`)
  h.cmd(`fill ${X - R} ${Y} ${Z - R} ${X + R} ${Y} ${Z + R} sand`)
  h.cmd('gamerule mob_griefing true')
  await h.ticks(10)

  await reset()
  h.cmd(`summon turtle ${X + 0.5} ${Y + 1} ${Z + 0.5} {has_egg:1b,home_pos:[I;${X},${Y + 1},${Z}]}`)
  await h.waitFor(() => turtles().length === 1, 5000)
  await h.give('seagrass', 2)
  bot.setQuickBarSlot(0)
  bot._client.write('use_entity', { target: turtles()[0].id, hand: 0, location: { x: 0, y: 0.2, z: 0 }, sneaking: false })
  await h.ticks(4)
  h.result('egg.feed.seagrass', h.inv().seagrass || 0)
  // Out of the turtle's way, but close enough to see it.
  h.cmd(`tp Keeper ${X + R + 1.5} ${Y + 1} ${Z + 0.5}`)
  await h.serverTicks(100)
  state('lay.100')

  await reset()
  // Summoned with NBT, vanilla skips finalizeSpawn and reads the home before the position is set.
  const home = `home_pos:[I;${X},${Y + 1},${Z}]`
  h.cmd(`summon turtle ${X - 0.5} ${Y + 1} ${Z + 0.5} {InLove:600,${home}}`)
  h.cmd(`summon turtle ${X + 1.5} ${Y + 1} ${Z + 0.5} {InLove:600,${home}}`)
  await h.waitFor(() => turtles().length === 2, 5000)
  await h.serverTicks(100)
  state('breed.100')
  h.cmd('kill @e[type=turtle]')
})
