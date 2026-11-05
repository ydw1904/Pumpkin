// Armadillos roll up: from a hit by a living attacker, from undead and sprinting players nearby,
// and roll out once the danger is forgotten or environmental damage makes them panic. Brushing
// one drops a scute. Records the states the client sees and the armadillo sounds it hears.
const { make } = require('./survival-lib')
const h = make('Roller')
const { bot } = h
const X = 9600; const Y = 200; const Z = 0
const STATE = 18
const NAMES = ['idle', 'rolling', 'scared', 'unrolling']

// Sound ids as this client's registry numbers them, without the armadillo's random ambient and
// step sounds (57 and 58 in 26.3).
const IGNORED_SOUNDS = new Set([57, 58])
const heard = []
bot._client.on('sound_effect', (p) => {
  const id = p.sound && p.sound.soundId
  if (id !== undefined && !IGNORED_SOUNDS.has(id)) heard.push(id)
})

const armadillos = () => Object.values(bot.entities).filter(e => e.name === 'armadillo')
const stateOf = (e) => { const v = e.metadata[STATE]; return NAMES[v] ?? (v === undefined ? 'idle' : String(v)) }

async function summon (dx, dz) {
  h.cmd('kill @e[type=!player]')
  await h.ticks(5)
  const old = new Set(armadillos().map(e => e.id))
  h.cmd(`summon armadillo ${X + 0.5 + dx} ${Y + 1} ${Z + 0.5 + dz} {PersistenceRequired:1b}`)
  let e
  await h.waitFor(() => (e = armadillos().find(m => !old.has(m.id))), 5000)
  await h.ticks(20)
  heard.length = 0
  return e
}

// The distinct states in order and the ticks until the armadillo is idle again, rounded to 10.
async function follow (e, ticks) {
  const states = [stateOf(e)]
  let idleAt = null
  let n = 0
  const sample = () => {
    n++
    const s = stateOf(e)
    if (s !== states[states.length - 1]) {
      states.push(s)
      if (s === 'idle') idleAt = n
    }
  }
  bot.on('physicsTick', sample)
  await h.ticks(ticks)
  bot.removeListener('physicsTick', sample)
  return { states: states.join('>'), idle: idleAt === null ? 'never' : Math.round(idleAt / 20) * 20 }
}

h.start(async () => {
  await h.arena(X, Y, Z, 12)
  h.cmd(`fill ${X - 12} ${Y + 1} ${Z - 12} ${X + 12} ${Y + 2} ${Z + 12} glass hollow`)
  h.cmd(`fill ${X - 11} ${Y + 1} ${Z - 11} ${X + 11} ${Y + 2} ${Z + 11} air`)
  h.cmd('time set noon')
  h.cmd('gamerule advance_time false')
  h.cmd(`tp ${bot.username} ${X + 0.5} ${Y + 1} ${Z + 10.5}`)
  await h.ticks(20)
  // Client ticks per 100 server ticks, to tell a slow or catching up server from a timing bug.
  const age = bot.time.age
  let clientTicks = 0
  const count = () => clientTicks++
  bot.on('physicsTick', count)
  await h.waitFor(() => bot.time.age - age >= 100, 20000)
  bot.removeListener('physicsTick', count)
  console.log(`INFO client ticks per 100 server ticks: ${clientTicks}`)

  // A hit by the bot from 10 blocks away: out of scare range, so only the hit's memory counts.
  let e = await summon(0, 0)
  h.cmd(`damage @e[type=armadillo,limit=1] 1 minecraft:player_attack by ${bot.username}`)
  let r = await follow(e, 400)
  h.result('player_hit.states', r.states)
  h.result('player_hit.idle_after_20s', r.idle)
  h.result('player_hit.sounds', heard.join(','))

  // A hit while rolled up sounds reduced; fire damage then rolls it out.
  e = await summon(0, 0)
  h.cmd(`damage @e[type=armadillo,limit=1] 1 minecraft:player_attack by ${bot.username}`)
  await h.ticks(30)
  heard.length = 0
  h.cmd(`damage @e[type=armadillo,limit=1] 3 minecraft:player_attack by ${bot.username}`)
  await h.ticks(10)
  h.result('scared_hit.sounds', heard.join(','))
  heard.length = 0
  h.cmd('damage @e[type=armadillo,limit=1] 1 minecraft:in_fire')
  r = await follow(e, 40)
  h.result('fire_while_scared.states', r.states)
  h.result('fire_while_scared.sounds', heard.join(','))

  // Fire damage alone never rolls it up.
  e = await summon(0, 0)
  h.cmd('damage @e[type=armadillo,limit=1] 1 minecraft:in_fire')
  r = await follow(e, 60)
  h.result('fire.states', r.states)

  // An undead mob within 7 blocks scares it until it is gone.
  e = await summon(0, 0)
  h.cmd(`summon zombie ${X + 3.5} ${Y + 1} ${Z + 0.5} {NoAI:1b,PersistenceRequired:1b,Silent:1b}`)
  r = await follow(e, 100)
  h.result('undead.states', r.states)
  h.cmd('kill @e[type=zombie]')
  r = await follow(e, 300)
  h.result('undead_gone.states', r.states)
  h.result('undead_gone.idle_after_20s', r.idle)

  // A sprinting player scares it; walking past doesn't.
  e = await summon(0, 0)
  h.cmd(`tp ${bot.username} ${X + 0.5} ${Y + 1} ${Z + 5.5} 180 0`)
  await h.ticks(20)
  bot.setControlState('forward', true)
  r = await follow(e, 30)
  bot.setControlState('forward', false)
  h.result('walk.states', r.states)
  h.cmd(`tp ${bot.username} ${X + 0.5} ${Y + 1} ${Z + 5.5} 180 0`)
  await h.ticks(20)
  bot.setControlState('sprint', true)
  bot.setControlState('forward', true)
  r = await follow(e, 30)
  bot.setControlState('forward', false)
  bot.setControlState('sprint', false)
  h.result('sprint.states', r.states)

  // Brushing drops a scute and costs the brush 16 durability.
  e = await summon(0, 0)
  h.cmd(`tp ${bot.username} ${X + 0.5} ${Y + 1} ${Z + 2.5} 180 20`)
  await h.ticks(20)
  await h.clear()
  await h.give('brush')
  bot.setQuickBarSlot(0)
  await h.ticks(5)
  await bot.activateEntity(e)
  await h.ticks(10)
  const scutes = Object.values(bot.entities).filter(i => i.name === 'item').length
  h.result('brush.items_dropped', scutes)
  const brush = bot.inventory.slots[36]
  h.result('brush.damage', brush ? (brush.durabilityUsed ?? brush.nbt ?? 'unknown') : 'gone')
  h.result('brush.sounds', heard.join(','))
  h.result('brush.state', stateOf(e))
  h.cmd('kill @e[type=!player]')
})
