// An arrow summoned above a survival player falls onto them: it should hurt the player.
const mineflayer = require('mineflayer')
const fs = require('fs')

const PORT = Number(process.env.PORT)
const CMDS = process.env.CMDS
const result = (key, value) => console.log(`RESULT ${key} ${value}`)
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms))
const command = (line) => fs.appendFileSync(CMDS, line + '\n')

const bot = mineflayer.createBot({ host: '127.0.0.1', port: PORT, username: 'Archer', version: '26.3' })
bot.on('error', (err) => console.log('ERROR', err.message))

bot.once('spawn', async () => {
  // Creative until the platform is there: the chunk may not be loaded yet when /fill first runs.
  command('gamemode creative Archer')
  command('difficulty normal')
  command('forceload add 0 0')
  command('tp Archer 0.5 200 0.5')
  const ready = Date.now() + 60000
  while (Date.now() < ready) {
    command('fill -2 199 -2 2 199 2 minecraft:stone')
    command('tp Archer 0.5 200 0.5')
    await sleep(1000)
    const below = bot.blockAt(bot.entity.position.offset(0, -1, 0))
    if (below && below.name === 'stone' && bot.entity.onGround) break
  }
  result('platform_ready', Date.now() < ready)
  command('gamemode survival Archer')
  await sleep(2000)
  const healthBefore = bot.health
  result('health_before', healthBefore)
  command('summon minecraft:arrow 0.5 204 0.5 {Motion:[0.0,-1.5,0.0]}')
  const end = Date.now() + 5000
  while (Date.now() < end && bot.health >= healthBefore) await sleep(50)
  result('hurt_by_arrow', bot.health < healthBefore)
  // An arrow that hurts a non-living target, a minecart, still plays its hit sound.
  const sounds = []
  bot.on('hardcodedSoundEffectHeard', (id, category, pos) => { if (pos && Math.abs(pos.x - 6.5) < 2) sounds.push(id) })
  bot.on('soundEffectHeard', (name, pos) => { if (pos && Math.abs(pos.x - 6.5) < 2) sounds.push(name) })
  command('summon minecart 6.5 200 0.5 {NoGravity:1b}')
  await sleep(1000)
  command('summon minecraft:arrow 6.5 204 0.5 {Motion:[0.0,-1.5,0.0]}')
  await sleep(3000)
  result('minecart_hit_sounds', sounds.join(',') || 'none')
  // An arrow Archer owns hurting another player sends Archer the arrow-hit-player event.
  const target = mineflayer.createBot({ host: '127.0.0.1', port: PORT, username: 'Target', version: '26.3' })
  await new Promise((resolve) => target.once('spawn', resolve))
  command('gamemode survival Target')
  command('tp Target 1.5 200 1.5')
  await sleep(2000)
  const events = []
  bot._client.on('game_state_change', (p) => events.push(p.reason))
  const ints = bot.player.uuid.replace(/-/g, '').match(/.{8}/g).map((s) => parseInt(s, 16) | 0)
  command(`summon minecraft:arrow 1.5 204 1.5 {Motion:[0.0,-1.5,0.0],Owner:[I;${ints.join(',')}]}`)
  await sleep(3000)
  result('target_health', target.health)
  result('game_events', events.join(',') || 'none')
  target.quit()
  bot.quit()
  setTimeout(() => process.exit(0), 1000)
})
