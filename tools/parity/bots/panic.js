// Which damage makes a mob panic: wolves, armadillos and adult polar bears flee only from
// environmental damage, baby polar bears from any hit. A fleeing mob runs at 1.5 to 2 times its
// wandering speed, so the fastest 5 tick stretch after the knockback has settled tells a flight
// apart from standing or wandering.
const { make } = require('./survival-lib')
const h = make('Spooker')
const { bot } = h
const X = 9400; const Y = 200; const Z = 0
const R = 12

const cases = [
  // [label, type, nbt, blocks per 5 ticks that only a fleeing mob reaches]
  ['wolf', 'wolf', '', 1.1],
  ['polar_bear', 'polar_bear', '', 1.2],
  ['baby_polar_bear', 'polar_bear', ',Age:-24000', 0.9],
  ['armadillo', 'armadillo', '', 0.38],
]

async function fastest (type, nbt, damage) {
  h.cmd('kill @e[type=!player]')
  await h.ticks(5)
  const old = new Set(Object.values(bot.entities).filter(e => e.name === type).map(e => e.id))
  h.cmd(`summon ${type} ${X + 0.5} ${Y + 1} ${Z + 0.5} {PersistenceRequired:1b,Silent:1b${nbt}}`)
  let e
  await h.waitFor(() => (e = Object.values(bot.entities).find(m => m.name === type && !old.has(m.id))), 5000)
  if (!e) return 'no mob'
  await h.ticks(20)
  h.cmd(`damage @e[type=${type},limit=1] 1 ${damage}`)
  await h.ticks(10)
  const track = []
  const sample = () => track.push(e.position.clone())
  bot.on('physicsTick', sample)
  await h.ticks(30)
  bot.removeListener('physicsTick', sample)
  let best = 0
  for (let i = 5; i < track.length; i++) {
    const a = track[i - 5]; const b = track[i]
    best = Math.max(best, Math.hypot(b.x - a.x, b.z - a.z))
  }
  return best
}

h.start(async () => {
  await h.arena(X, Y, Z, R)
  h.cmd(`fill ${X - R} ${Y + 1} ${Z - R} ${X + R} ${Y + 3} ${Z + R} glass hollow`)
  h.cmd(`fill ${X - R + 1} ${Y + 1} ${Z - R + 1} ${X + R - 1} ${Y + 3} ${Z + R - 1} air`)
  h.cmd(`fill ${X - R} ${Y + 3} ${Z - R} ${X + R} ${Y + 3} ${Z + R} air`)
  // Creative, so no mob goes after the bot that hits it.
  h.cmd(`gamemode creative ${bot.username}`)
  h.cmd(`tp ${bot.username} ${X + 0.5} ${Y + 1} ${Z + R - 2.5}`)
  await h.ticks(20)
  for (const [label, type, nbt, threshold] of cases) {
    for (const [cause, damage] of [['in_fire', 'minecraft:in_fire'], ['player', `minecraft:player_attack by ${bot.username}`]]) {
      // Three tries, since a wander can start in the window too.
      let fled = 0
      for (let i = 0; i < 3; i++) {
        const speed = await fastest(type, nbt, damage)
        if (typeof speed === 'number' && speed > threshold) fled++
      }
      h.result(`${label}.${cause}.fled`, fled >= 2)
    }
  }
  h.cmd('kill @e[type=!player]')
})
