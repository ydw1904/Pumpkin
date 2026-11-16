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
const achieved = new Set()
bot._client.on('advancements', (packet) => {
  for (const entry of packet.progressMapping || []) {
    if ((entry.value || []).some((c) => c.criterionProgress !== undefined && c.criterionProgress !== null)) achieved.add(entry.key)
  }
})

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
  // The arrow that hurt Target stays stuck in it.
  const targetEntity = Object.values(bot.entities).find((e) => e.username === 'Target')
  result('target_arrows', targetEntity ? targetEntity.metadata[12] ?? 0 : 'none')
  target.quit()
  await sleep(1000)
  // A Punch crossbow arrow knocks a husk back and plays the crossbow hit sound.
  const huskSounds = []
  const soundName = (id) => (bot.registry.sounds && bot.registry.sounds[id] && bot.registry.sounds[id].name) || id
  bot.on('hardcodedSoundEffectHeard', (id, category, pos) => { if (pos && pos.z > 2) huskSounds.push(soundName(id)) })
  bot.on('soundEffectHeard', (name, pos) => { if (pos && pos.z > 2) huskSounds.push(name) })
  const huskVelocities = []
  command('summon husk 0.5 200 3.5 {NoAI:1b,Silent:1b}')
  command('give Archer crossbow[charged_projectiles=[{id:"minecraft:arrow",count:1}],enchantments={"minecraft:punch":2}]')
  await sleep(1500)
  const husk = Object.values(bot.entities).find((e) => e.name === 'husk')
  bot._client.on('entity_velocity', (p) => {
    // The shot's spread is random, so only the push along the flight and up is compared.
    if (husk && p.entityId === husk.id) huskVelocities.push([Math.hypot(p.velocity.x, p.velocity.z), p.velocity.y].map((n) => Math.round(n * 100) / 100).join(','))
  })
  const crossbow = bot.inventory.items().find((i) => i.name === 'crossbow')
  if (husk && crossbow) {
    await bot.equip(crossbow, 'hand')
    await bot.lookAt(husk.position.offset(0, 1.5, 0), true)
    bot.activateItem()
    await sleep(2000)
  }
  result('punch_knockback', huskVelocities[0] || 'none')
  result('crossbow_hit_sounds', huskSounds.join(',') || 'none')
  // An arrow its shooter hurt something with grants "Take Aim", not "A Throwaway Joke".
  result('shoot_arrow_advancement', achieved.has('minecraft:adventure/shoot_arrow'))
  result('throw_trident_advancement', achieved.has('minecraft:adventure/throw_trident'))
  // A fast arrow passes fire 17 travel steps into its flight: vanilla applies block effects for
  // only 16 steps a tick, so it flies on unlit.
  command('weather clear')
  command('setblock 12 200 12 minecraft:netherrack')
  command('setblock 12 201 12 minecraft:fire')
  await sleep(1000)
  let fastArrowLit = false
  bot._client.on('entity_metadata', (p) => {
    const e = bot.entities[p.entityId]
    if (!e || e.name !== 'arrow' || e.position.z < 2) return
    for (const m of p.metadata) if (m.key === 0 && (m.value & 1)) fastArrowLit = true
  })
  command('summon minecraft:arrow 3.5 201.2 3.5 {Motion:[9.9,0.0,9.9]}')
  await sleep(2000)
  result('fast_arrow_lit', fastArrowLit)
  // An arrow stuck on top of a block falls once the block becomes one without collision, and
  // stays while the block only changes to another solid one.
  const arrowAt = (x) => Object.values(bot.entities).find((e) => e.name === 'arrow' && Math.abs(e.position.x - x) < 0.6 && Math.abs(e.position.z + 3.5) < 0.6)
  const arrowY = (x) => { const e = arrowAt(x); return e ? e.position.y.toFixed(1) : 'none' }
  for (const x of [8, 10]) {
    command(`fill ${x - 1} 199 -5 ${x + 1} 199 -3 minecraft:stone`)
    command(`setblock ${x} 201 -4 minecraft:stone`)
  }
  await sleep(1000)
  for (const x of [8, 10]) command(`summon minecraft:arrow ${x + 0.5} 205 -3.5 {Motion:[0.0,-1.0,0.0]}`)
  await sleep(2000)
  result('stuck_arrow_y', arrowY(8.5))
  command('setblock 8 201 -4 minecraft:dirt')
  command('setblock 10 201 -4 minecraft:light')
  await sleep(2000)
  result('arrow_on_new_block_y', arrowY(8.5))
  result('arrow_on_light_y', arrowY(10.5))
  bot.quit()
  setTimeout(() => process.exit(0), 1000)
})
