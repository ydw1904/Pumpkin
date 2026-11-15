// Blocks a projectile flies through act on it (vanilla `applyEffectsFromBlocks`), and the world
// border stops projectiles: arrows bounce back off it, thrown items break on it.
const { make } = require('./survival-lib')
const h = make('Blocks')
const { bot } = h
const X = 0; const Y = 200; const Z = 0

// Highest and last position the bot saw of the first entity of `type` spawned after `summon`.
async function follow (type, summon, ms) {
  let tracked = null
  const onSpawn = (e) => { if (e.name === type && tracked === null) tracked = e }
  bot.on('entitySpawn', onSpawn)
  h.cmd(summon)
  await h.waitFor(() => tracked !== null, 5000)
  bot.removeListener('entitySpawn', onSpawn)
  if (!tracked) return null
  let maxY = tracked.position.y; let maxX = tracked.position.x
  const end = Date.now() + ms
  while (Date.now() < end) {
    maxY = Math.max(maxY, tracked.position.y)
    maxX = Math.max(maxX, tracked.position.x)
    await h.sleep(50)
  }
  return { maxY, maxX, last: tracked.position.clone(), alive: bot.entities[tracked.id] !== undefined, entity: tracked }
}

h.start(async () => {
  await h.arena(X, Y, Z, 4)
  h.cmd(`gamemode creative ${bot.username}`)
  h.cmd('weather clear')

  // An upward bubble column of 8 blocks walled in glass, open to the air at the top.
  const cx = X + 8; const cz = Z
  h.cmd(`fill ${cx - 1} ${Y - 10} ${cz - 1} ${cx + 1} ${Y - 1} ${cz + 1} glass`)
  h.cmd(`setblock ${cx} ${Y - 10} ${cz} soul_sand`)
  h.cmd(`fill ${cx} ${Y - 9} ${cz} ${cx} ${Y - 1} ${cz} bubble_column[drag=false]`)
  await h.waitFor(() => h.blockName(cx, Y - 1, cz) === 'bubble_column', 5000)
  await h.sleep(1500)
  h.result('column_top', h.blockName(cx, Y - 1, cz))

  // A snowball let go inside the column is carried up and out of it instead of sinking.
  const snowball = await follow('snowball', `summon snowball ${cx + 0.5} ${Y - 6} ${cz + 0.5} {Motion:[0d,0d,0d]}`, 3000)
  h.result('snowball_in_column.max_y_above_start', snowball ? Math.floor(snowball.maxY - (Y - 6)) : 'none')
  h.result('snowball_in_column.left_column_top', snowball ? snowball.maxY > Y : 'none')
  h.cmd('kill @e[type=snowball]')

  // A downward column holds an arrow under instead.
  const dx = X - 8
  h.cmd(`fill ${dx - 1} ${Y - 10} ${cz - 1} ${dx + 1} ${Y - 1} ${cz + 1} glass`)
  h.cmd(`setblock ${dx} ${Y - 10} ${cz} magma_block`)
  h.cmd(`fill ${dx} ${Y - 9} ${cz} ${dx} ${Y - 1} ${cz} bubble_column[drag=true]`)
  await h.waitFor(() => h.blockName(dx, Y - 1, cz) === 'bubble_column', 5000)
  await h.sleep(1500)
  const arrowDown = await follow('arrow', `summon arrow ${dx + 0.5} ${Y + 1} ${cz + 0.5} {Motion:[0d,-0.6d,0d]}`, 3000)
  h.result('arrow_down_column.rest_y', arrowDown ? Math.floor(arrowDown.last.y) : 'none')
  h.cmd('kill @e[type=arrow]')

  // An arrow flying down through fire catches fire.
  const fx = X + 3; const fz = Z + 3
  h.cmd(`setblock ${fx} ${Y} ${fz} netherrack`)
  h.cmd(`setblock ${fx} ${Y + 1} ${fz} fire`)
  await h.waitFor(() => h.blockName(fx, Y + 1, fz) === 'fire', 3000)
  const burning = await follow('arrow', `summon arrow ${fx + 0.5} ${Y + 4} ${fz + 0.5} {Motion:[0d,-0.5d,0d]}`, 2500)
  const flags = burning ? burning.entity.metadata[0] : undefined
  h.result('arrow_through_fire.on_fire', flags === undefined ? 'none' : (flags & 1) === 1)
  h.cmd('kill @e[type=arrow]')
  h.cmd(`setblock ${fx} ${Y + 1} ${fz} air`)

  // The world border: an arrow bounces back off it, a snowball breaks on it.
  h.cmd(`worldborder center ${X} ${Z}`)
  h.cmd('worldborder set 40')
  await h.sleep(1000)
  const arrow = await follow('arrow', `summon arrow ${X + 17.5} ${Y + 3} ${Z + 0.5} {Motion:[1.5d,0d,0d],NoGravity:1b}`, 2500)
  h.result('arrow_at_border.passed_border', arrow ? arrow.maxX > X + 20.5 : 'none')
  h.result('arrow_at_border.came_back', arrow ? arrow.last.x < X + 19.5 : 'none')
  h.cmd('kill @e[type=arrow]')
  const ball = await follow('snowball', `summon snowball ${X + 17.5} ${Y + 3} ${Z + 0.5} {Motion:[1.5d,0d,0d],NoGravity:1b}`, 2500)
  h.result('snowball_at_border.passed_border', ball ? ball.maxX > X + 20.5 : 'none')
  h.result('snowball_at_border.alive', ball ? ball.alive : 'none')
  h.cmd('kill @e[type=snowball]')
  h.cmd('worldborder set 59999968')
})
