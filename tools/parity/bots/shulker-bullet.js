// Shulker bullets: one homing in on the player hurts it and makes it levitate, one flying into a
// wall bursts, one hitting a minecart is used up on it, one hitting a ghast fireball turns the
// fireball the way the bullet flew, and an arrow destroys a bullet.
const { make } = require('./survival-lib')
const h = make('Bulleted')
const { bot } = h
const X = 9600; const Y = 200; const Z = -600

const r2 = (n) => Math.round(n * 100) / 100
const r3 = (n) => Math.round(n * 1000) / 1000
const velocities = {}
const gone = {}
const sounds = []
bot._client.on('entity_velocity', (p) => {
  const v = p.velocity
  ;(velocities[p.entityId] ||= []).push([r3(v.x), r3(v.y), r3(v.z)].join(','))
})
bot.on('entityGone', (e) => { gone[e.id] = `${r2(e.position.x - X)},${r2(e.position.y - Y)},${r2(e.position.z - Z)}` })
const soundName = (id) => (bot.registry.sounds && bot.registry.sounds[id] && bot.registry.sounds[id].name) || id
bot.on('hardcodedSoundEffectHeard', (id) => sounds.push(soundName(id)))
bot.on('soundEffectHeard', (name) => sounds.push(name))
const near = (name) => Object.values(bot.entities)
  .filter(e => e.name === name && e.position.distanceTo(bot.entity.position) < 30)[0]

async function reset () {
  h.cmd(`kill @e[type=!player,distance=..40,x=${X},y=${Y},z=${Z}]`)
  h.cmd(`effect clear ${bot.username}`)
  h.cmd(`effect give ${bot.username} instant_health 1 5`)
  h.cmd(`fill ${X - 4} ${Y + 1} ${Z + 11} ${X + 4} ${Y + 6} ${Z + 11} stone`)
  await h.tp(X + 0.5, Y + 1, Z + 0.5, 0, 0)
  await h.serverTicks(40)
}

async function spawned (name) {
  await h.waitFor(() => near(name), 5000)
  return near(name)
}

// Sounds heard from `from` on, without the player's own hurt and step sounds.
const heard = (from) => sounds.slice(from).filter(s => !/player|step/.test(String(s))).join(',') || 'none'

h.start(async () => {
  await h.arena(X, Y, Z, 12)
  h.cmd('difficulty normal')
  h.cmd('gamerule fall_damage false')
  h.cmd('gamerule mob_griefing false')

  // A bullet aimed at the player homes in, hurts it and makes it levitate. A summoned bullet
  // only picks its first direction once its flight steps run out.
  await reset()
  const health = bot.health
  const ints = bot.player.uuid.replace(/-/g, '').match(/.{8}/g).map((s) => parseInt(s, 16) | 0)
  h.cmd(`summon shulker_bullet ${X + 0.5} ${Y + 4} ${Z + 6.5} {Target:[I;${ints.join(',')}],Steps:1}`)
  await h.waitFor(() => bot.health < health, 10000)
  await h.serverTicks(5)
  h.result('homing.hurt', r2(health - bot.health))
  h.result('homing.effects', Object.keys(bot.entity.effects || {}).sort().join(',') || 'none')

  // A bullet flying into the wall bursts there.
  await reset()
  let s0 = sounds.length
  h.cmd(`summon shulker_bullet ${X + 0.5} ${Y + 2.5} ${Z + 3.5} {Motion:[0d,0d,0.5d],NoGravity:1b}`)
  let bullet = await spawned('shulker_bullet')
  await h.serverTicks(30)
  h.result('wall.gone', bullet ? gone[bullet.id] || 'flying' : 'none')
  h.result('wall.sounds', heard(s0))

  // A bullet is used up on a minecart in its way.
  await reset()
  h.cmd(`summon minecart ${X + 0.5} ${Y + 2} ${Z + 7.5} {NoGravity:1b}`)
  await spawned('minecart')
  h.cmd(`summon shulker_bullet ${X + 0.5} ${Y + 2.3} ${Z + 3.5} {Motion:[0d,0d,0.5d],NoGravity:1b}`)
  bullet = await spawned('shulker_bullet')
  await h.serverTicks(30)
  h.result('minecart.gone', bullet ? gone[bullet.id] || 'flying' : 'none')

  // A ghast fireball the bullet hits flies on the way the bullet moved.
  await reset()
  h.cmd(`summon fireball ${X + 0.5} ${Y + 2.5} ${Z + 7.5} {Motion:[0d,0d,0d],acceleration_power:0d}`)
  const fireball = await spawned('fireball')
  h.cmd(`summon shulker_bullet ${X + 0.5} ${Y + 2.5} ${Z + 2.5} {Motion:[0d,0d,0.5d],NoGravity:1b}`)
  bullet = await spawned('shulker_bullet')
  await h.serverTicks(30)
  h.result('fireball.bullet_gone', bullet ? gone[bullet.id] || 'flying' : 'none')
  h.result('fireball.velocities', fireball ? (velocities[fireball.id] || []).filter(v => v !== '0,0,0').slice(0, 2).join(' ') || 'none' : 'none')

  // An arrow destroys a bullet in its way.
  await reset()
  s0 = sounds.length
  h.cmd(`summon shulker_bullet ${X + 0.5} ${Y + 2.5} ${Z + 7.5} {NoGravity:1b}`)
  bullet = await spawned('shulker_bullet')
  h.cmd(`summon arrow ${X + 0.5} ${Y + 2.5} ${Z + 2.5} {Motion:[0d,0d,1d],NoGravity:1b,pickup:0b}`)
  await h.serverTicks(30)
  h.result('shot.gone', bullet ? gone[bullet.id] || 'flying' : 'none')
  h.result('shot.sounds', heard(s0))
  h.cmd(`kill @e[type=!player,distance=..40,x=${X},y=${Y},z=${Z}]`)
})
