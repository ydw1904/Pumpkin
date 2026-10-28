// Login, chunk loading, commands, movement and fall damage as a headless 26.3 client sees them.
// Prints RESULT lines for bot_parity.py to compare between vanilla and Pumpkin.
const fs = require('fs')
const mineflayer = require('mineflayer')
const { Vec3 } = require('vec3')

const PLATFORM_Y = 199
const bot = mineflayer.createBot({
  host: '127.0.0.1',
  port: Number(process.env.PORT),
  username: 'Smoke',
  version: '26.3',
  auth: 'offline'
})

const result = (key, value) => console.log(`RESULT ${key} ${value}`)
const consoleCmd = (cmd) => fs.appendFileSync(process.env.CMDS, cmd + '\n')
const ticks = (n) => bot.waitForTicks(n)
const blockName = (x, y, z) => { const b = bot.blockAt(new Vec3(x, y, z)); return b ? b.name : 'unloaded' }

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms))

// Polls on wall-clock time: bot ticks stop while the bot stands in a chunk it hasn't received.
async function waitFor (predicate, timeoutMs) {
  const end = Date.now() + timeoutMs
  while (Date.now() < end) {
    if (predicate()) return true
    await sleep(50)
  }
  return false
}

function nearbyChunksLoaded () {
  const { x, z } = bot.entity.position
  for (let dx = -1; dx <= 1; dx++) {
    for (let dz = -1; dz <= 1; dz++) {
      if (!bot.world.getColumnAt(new Vec3(x + dx * 16, 0, z + dz * 16))) return false
    }
  }
  return true
}

let forcedMoves = 0
bot.on('forcedMove', () => { forcedMoves++ })
bot.on('kicked', (reason) => { result('kicked', JSON.stringify(reason)); process.exit(1) })
bot.on('error', (err) => console.log('ERROR', err.message))
setTimeout(() => { result('finished', 'timeout'); process.exit(1) }, 240000)

bot.once('spawn', async () => {
  result('spawned', 'true')
  result('game_mode', bot.game.gameMode)
  result('dimension', bot.game.dimension)
  await waitFor(() => bot.health !== undefined, 5000)
  result('health', bot.health)
  result('food', bot.food)

  // Mineflayer's own waitForChunksToLoad gives up after 10s, too short for a fresh debug world.
  result('nearby_chunks_loaded', await waitFor(nearbyChunksLoaded, 120000))
  const spawn = bot.entity.position.floored()
  result('standing_on', blockName(spawn.x, spawn.y - 1, spawn.z))

  consoleCmd(`op ${bot.username}`)
  await ticks(20)
  bot.chat('/gamemode creative')
  result('creative_applied', await waitFor(() => bot.game.gameMode === 'creative', 5000))

  // Build a platform high above the terrain so walking and falling don't depend on worldgen.
  bot.chat(`/tp ${bot.username} 0.5 ${PLATFORM_Y + 1} 0.5`)
  await waitFor(() => bot.entity.position.distanceTo(new Vec3(0.5, PLATFORM_Y + 1, 0.5)) < 1, 5000)
  const tpStart = Date.now()
  result('platform_chunks_loaded', await waitFor(nearbyChunksLoaded, 120000))
  console.log(`teleport chunks took ${Date.now() - tpStart} ms`)
  bot.chat(`/fill -8 ${PLATFORM_Y} -8 8 ${PLATFORM_Y} 8 minecraft:stone`)
  result('fill_visible', await waitFor(() => blockName(8, PLATFORM_Y, 8) === 'stone' && blockName(-8, PLATFORM_Y, -8) === 'stone', 5000))
  bot.chat(`/setblock 0 ${PLATFORM_Y + 3} 0 minecraft:gold_block`)
  result('setblock_visible', await waitFor(() => blockName(0, PLATFORM_Y + 3, 0) === 'gold_block', 5000))
  bot.chat(`/setblock 0 ${PLATFORM_Y + 3} 0 minecraft:air`)
  await ticks(5)

  bot.chat('/gamemode survival')
  result('survival_applied', await waitFor(() => bot.game.gameMode === 'survival', 5000))
  bot.chat(`/tp ${bot.username} 0.5 ${PLATFORM_Y + 1} 0.5 0 0`)
  await waitFor(() => bot.entity.onGround, 5000)
  await ticks(10)

  // Walking is client authoritative; the server should accept it without pulling the bot back.
  const movesBefore = forcedMoves
  bot.setControlState('forward', true)
  await ticks(20)
  bot.setControlState('forward', false)
  await ticks(10)
  result('walk_corrections', forcedMoves - movesBefore)
  result('walked_onto', blockName(Math.floor(bot.entity.position.x), PLATFORM_Y, Math.floor(bot.entity.position.z)))

  // A 10 block fall onto stone deals 10 - 3 = 7 damage, worked out by the server. Read the first
  // health update after landing, before natural regeneration heals part of it.
  const healthBefore = bot.health
  let landedHealth = null
  const onHealth = () => { if (landedHealth === null && bot.health < healthBefore) landedHealth = bot.health }
  bot.on('health', onHealth)
  bot.chat(`/tp ${bot.username} 0.5 ${PLATFORM_Y + 11} 0.5`)
  await waitFor(() => bot.entity.position.y > PLATFORM_Y + 5, 5000)
  await waitFor(() => landedHealth !== null, 10000)
  bot.removeListener('health', onHealth)
  result('fall_damage', landedHealth === null ? 'none' : healthBefore - landedHealth)

  result('finished', 'true')
  bot.quit()
  setTimeout(() => process.exit(0), 1000)
})
