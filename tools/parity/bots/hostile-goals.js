// Hostile mob goals a client can see: the guardian beam (attack target data, charge time and
// damage), the elder guardian's mining fatigue, the vex charge flag, its missing gravity and
// limited life, the illusioner's spells and bow, and the vindicator's Johnny targeting.
const { make } = require('./survival-lib')
const h = make('Hunted', 900000)
const { bot } = h
const X = 3000; const Y = 200; const Z = 0
// The water tank sits apart from the arena.
const TX = X + 40
const GUARDIAN_ATTACK_TARGET = 17
const VEX_FLAGS = 16
const SPELL = 17
const NO_GRAVITY = 5

// Effects as `id:amplifier`, ids as this client numbers them.
const effects = () => Object.values(bot.entity.effects || {}).map(e => `${e.id}:${e.amplifier}`).sort().join(',')
const mobs = (name) => Object.values(bot.entities).filter(e => e.name === name)

// Every value an entity's metadata field takes while `fn` runs, sampled each client tick.
async function watch (entity, index, fn) {
  const seen = new Set()
  const sample = () => { const v = entity.metadata[index]; if (v !== undefined) seen.add(v) }
  bot.on('physicsTick', sample)
  try { await fn() } finally { bot.removeListener('physicsTick', sample) }
  return [...seen].sort().join(',')
}

async function reset () {
  h.cmd('kill @e[type=!player]')
  h.cmd(`effect clear ${bot.username}`)
  h.cmd(`effect give ${bot.username} instant_health 1 10`)
  await h.waitFor(() => bot.health === 20 && mobs('item').length === 0, 5000)
  h.cmd('kill @e[type=item]')
  await h.ticks(20)
}

async function summonAt (type, x, y, z, nbt) {
  // Only a new entity counts: a killed one can linger on the client for a moment.
  const old = new Set(mobs(type).map(m => m.id))
  h.cmd(`summon ${type} ${x} ${y} ${z} {PersistenceRequired:1b,Silent:1b${nbt ? ',' + nbt : ''}}`)
  let e
  await h.waitFor(() => (e = mobs(type).find(m => !old.has(m.id))), 5000)
  return e
}

const summon = (type, dx, dy, dz, nbt) => summonAt(type, X + 0.5 + dx, Y + 1 + dy, Z + 0.5 + dz, nbt)

// A glass tank the guardian and the bot share.
async function tank () {
  h.cmd(`fill ${TX - 6} ${Y} ${Z - 6} ${TX + 6} ${Y + 6} ${Z + 6} glass`)
  h.cmd(`fill ${TX - 5} ${Y + 1} ${Z - 5} ${TX + 5} ${Y + 6} ${Z + 5} water`)
  await h.waitFor(() => h.blockName(TX + 5, Y + 6, Z + 5) === 'water', 5000)
  await h.ticks(10)
}

async function enterTank () {
  h.cmd(`effect give ${bot.username} water_breathing infinite 0 true`)
  h.cmd(`tp ${bot.username} ${TX + 0.5} ${Y + 2} ${Z - 4.5} 0 0`)
  await h.waitFor(() => Math.abs(bot.entity.position.x - (TX + 0.5)) < 1, 5000)
  await h.ticks(10)
}

async function beam (kind, difficulty) {
  await reset()
  h.cmd(`difficulty ${difficulty}`)
  await enterTank()
  const g = await summonAt(kind, TX + 0.5, Y + 2, Z + 4.5)
  const start = bot.time.age
  const targeted = await h.waitFor(() => g.metadata[GUARDIAN_ATTACK_TARGET] === bot.entity.id, 15000)
  h.result(`${kind}.${difficulty}.beam_target`, targeted)
  if (!targeted) return
  const beamAt = bot.time.age
  // Client ticks from the beam to the hit; the world age only arrives every 20 ticks.
  let clientTicks = 0
  const count = () => clientTicks++
  bot.on('physicsTick', count)
  const before = bot.health
  await h.waitFor(() => bot.health < before, 8000)
  bot.removeListener('physicsTick', count)
  await h.ticks(5)
  // Rounded to 10 ticks, since packet timing jitters by a few.
  h.result(`${kind}.${difficulty}.charge_ticks_10s`, Math.round(clientTicks / 10) * 10)
  h.result(`${kind}.${difficulty}.damage`, Math.round((before - bot.health) * 10) / 10)
  await h.waitFor(() => !g.metadata[GUARDIAN_ATTACK_TARGET], 3000)
  h.result(`${kind}.${difficulty}.beam_cleared`, !g.metadata[GUARDIAN_ATTACK_TARGET])
  h.result(`${kind}.${difficulty}.found_within_15s`, beamAt - start < 300)
}

h.start(async () => {
  await h.arena(X, Y, Z, 8)
  h.cmd('gamerule natural_health_regeneration false')
  h.cmd('time set noon')
  h.cmd('gamerule advance_time false')

  await tank()
  await beam('guardian', 'normal')
  await beam('guardian', 'hard')
  await beam('elder_guardian', 'normal')

  // Mining fatigue every 1200 ticks, offset by the entity id. Resistance keeps the bot alive
  // through the beams meanwhile.
  await reset()
  h.cmd('difficulty normal')
  await enterTank()
  h.cmd(`effect give ${bot.username} resistance infinite 255 true`)
  await h.ticks(5)
  const plain = effects()
  await summonAt('elder_guardian', TX + 0.5, Y + 2, Z + 4.5)
  h.result('elder_guardian.fatigue', await h.waitFor(() => effects() !== plain, 75000))
  h.result('elder_guardian.effects', effects())
  await reset()
  h.cmd(`tp ${bot.username} ${X + 0.5} ${Y + 1} ${Z + 0.5}`)
  await h.waitFor(() => Math.abs(bot.entity.position.x - (X + 0.5)) < 1, 5000)
  await h.ticks(20)

  // Vex: no gravity, a charge flag while it dives, hits, and a limited life that starves it.
  h.cmd('difficulty normal')
  await reset()
  const vex = await summon('vex', 0, 2, 4)
  await h.ticks(5)
  h.result('vex.no_gravity', vex.metadata[NO_GRAVITY] === true)
  const before = bot.health
  let hit = false
  const flags = await watch(vex, VEX_FLAGS, async () => { hit = await h.waitFor(() => bot.health < before, 20000) })
  h.result('vex.hits', hit)
  h.result('vex.flags_seen', flags)
  await reset()
  const doomed = await summon('vex', 0, 2, 4, 'life_ticks:1,Health:1f,NoAI:1b')
  h.result('vex.limited_life_starves', await h.waitFor(() => !bot.entities[doomed.id], 3000))
  await reset()
  const kept = await summon('vex', 0, 2, 4, 'Health:1f,NoAI:1b')
  await h.ticks(40)
  h.result('vex.unlimited_life_stays', !!bot.entities[kept.id])
  await reset()

  // Illusioner: blindness needs a local difficulty above normal, so hard; mirror turns it
  // invisible; it shoots between spells.
  h.cmd('difficulty hard')
  h.cmd(`effect give ${bot.username} resistance infinite 255 true`)
  await h.ticks(5)
  const resisted = effects()
  const illusioner = await summon('illusioner', 0, 0, 6)
  let blinded = false
  const spells = await watch(illusioner, SPELL, async () => { blinded = await h.waitFor(() => effects() !== resisted, 30000) })
  h.result('illusioner.blinds', blinded)
  h.result('illusioner.effects', effects())
  h.result('illusioner.spells', spells)
  h.result('illusioner.shoots', await h.waitFor(() => mobs('arrow').length > 0, 15000))
  await reset()

  // Johnny: a vindicator saved as Johnny goes after any living thing, here a pig. A summoned
  // name alone doesn't count, since vanilla reads the name before the Johnny flag.
  h.cmd(`gamemode creative ${bot.username}`)
  await h.ticks(5)
  for (const [label, nbt] of [['johnny', 'Johnny:1b'], ['named', 'CustomName:"Johnny"'], ['plain', '']]) {
    await reset()
    await summon('vindicator', 0, 0, 4, nbt)
    const pig = await summon('pig', 0, 0, -4, 'NoAI:1b')
    await h.waitFor(() => !bot.entities[pig.id], 8000)
    h.result(`vindicator.${label}.kills_pig`, !bot.entities[pig.id])
  }
  h.cmd(`gamemode survival ${bot.username}`)
  await reset()
})
