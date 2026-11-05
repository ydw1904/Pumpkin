// Ambient sounds: mobs left alone play their ambient sound every few seconds, with the sound
// depending on their state (a drowned in water, a baby). Lists the sounds the bot heard.
const { make } = require('./survival-lib')
const h = make('Listener')
const { bot } = h
const X = 2800; const Y = 200; const Z = 0

// Mineflayer's sound names don't line up with the 26.3 registry, so this keeps the raw ids, which
// both servers share. Step and hurt sounds don't happen here: the mobs have no AI.
const heard = new Set()
bot._client.on('sound_effect', (p) => {
  const s = p.sound
  heard.add(s && s.data ? s.data.soundName : String(s ? s.soundId : p.soundId))
})
const take = () => { const s = [...heard].sort().join(',') || 'none'; heard.clear(); return s }

async function listen (key, mobs, ticks = 600) {
  for (const [i, m] of mobs.entries()) {
    h.cmd(`summon ${m.type} ${X - 4 + i * 2 + 0.5} ${m.y ?? Y + 1} ${Z + 3.5} {PersistenceRequired:1b,NoAI:1b${m.nbt ? ',' + m.nbt : ''}}`)
  }
  await h.ticks(20)
  take()
  await h.ticks(ticks)
  h.result(key, take())
  h.cmd('kill @e[type=!player]')
  await h.ticks(20)
}

h.start(async () => {
  await h.arena(X, Y, Z, 8)
  h.cmd('time set midnight')
  h.cmd('weather clear')
  h.cmd('gamerule advance_time false')
  h.cmd(`effect give ${bot.username} invisibility infinite 0 true`)
  await h.ticks(10)

  await listen('undead', [{ type: 'zombie' }, { type: 'husk' }, { type: 'skeleton' }, { type: 'zombie_villager' }])
  await listen('monsters', [{ type: 'spider' }, { type: 'enderman' }, { type: 'witch' }, { type: 'blaze' }])
  await listen('animals', [{ type: 'pig' }, { type: 'sheep' }, { type: 'wolf' }, { type: 'polar_bear', nbt: 'Age:-24000' }], 900)
  // A drowned in water and a guardian on land.
  h.cmd(`fill ${X - 5} ${Y + 1} ${Z + 2} ${X - 3} ${Y + 3} ${Z + 4} water`)
  await listen('water', [{ type: 'drowned', y: Y + 1 }, { type: 'guardian' }])
  h.cmd(`fill ${X - 5} ${Y + 1} ${Z + 2} ${X - 3} ${Y + 3} ${Z + 4} air`)
  h.result('finished', 'ok')
  process.exit(0)
})
