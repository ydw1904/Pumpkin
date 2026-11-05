// Slimes and magma cubes: what a killed cube splits into, how high each kind jumps, and that
// both leave a Peaceful world.
const { make } = require('./survival-lib')
const h = make('Cubes')
const { bot } = h
const X = 9900; const Y = 200; const Z = 0

const cubes = (name) => Object.values(bot.entities).filter(e => e.name === name)
const meta = (e) => e.metadata
  .map((v, k) => (v === undefined || v === null ? null : `${k}=${typeof v === 'object' ? JSON.stringify(v) : v}`))
  .filter(Boolean).join(' ')

async function split (name) {
  h.cmd('kill @e[type=!player]')
  await h.ticks(10)
  h.cmd(`summon ${name} ${X + 0.5} ${Y + 1} ${Z + 3.5} {Size:3,NoAI:1b,Silent:1b,PersistenceRequired:1b,Rotation:[0f,0f]}`)
  await h.waitFor(() => cubes(name).length === 1, 5000)
  await h.ticks(5)
  h.result(`${name}.parent`, meta(cubes(name)[0]))
  h.cmd(`kill @e[type=${name}]`)
  // The parent splits when it is removed after its 20 tick death animation. Waits on wall-clock
  // time because a lagging server falls behind the bot's own ticks.
  await h.waitFor(() => cubes(name).length >= 2, 15000)
  await h.ticks(5)
  const children = cubes(name)
  h.result(`${name}.children_in_range`, children.length >= 2 && children.length <= 4)
  h.result(`${name}.children`, [...new Set(children.map(meta))].sort().join(' | ') || 'none')
}

async function jump (name) {
  h.cmd('kill @e[type=!player]')
  await h.ticks(10)
  h.cmd(`summon ${name} ${X + 0.5} ${Y + 1} ${Z + 0.5} {Size:3,Silent:1b,PersistenceRequired:1b}`)
  await h.waitFor(() => cubes(name).length === 1, 5000)
  let peak = 0
  for (let i = 0; i < 300; i++) {
    await h.ticks(1)
    const cube = cubes(name)[0]
    if (cube) peak = Math.max(peak, cube.position.y - (Y + 1))
  }
  h.result(`${name}.jump_peak`, peak.toFixed(1))
}

h.start(async () => {
  await h.arena(X, Y, Z, 6)
  h.cmd(`gamemode creative ${bot.username}`)
  h.cmd('time set noon')
  h.cmd('gamerule advance_time false')
  h.cmd('difficulty normal')
  await h.ticks(10)
  for (const name of ['slime', 'magma_cube']) await split(name)
  for (const name of ['slime', 'magma_cube']) await jump(name)

  h.cmd('kill @e[type=!player]')
  await h.ticks(10)
  h.cmd(`summon slime ${X + 0.5} ${Y + 1} ${Z + 3.5} {Size:1}`)
  h.cmd(`summon magma_cube ${X + 0.5} ${Y + 1} ${Z - 2.5} {Size:1}`)
  await h.waitFor(() => cubes('slime').length + cubes('magma_cube').length === 2, 5000)
  h.cmd('difficulty peaceful')
  await h.ticks(20)
  h.result('peaceful.left', cubes('slime').length + cubes('magma_cube').length)
  h.cmd('difficulty normal')
  h.cmd('kill @e[type=!player]')
})
