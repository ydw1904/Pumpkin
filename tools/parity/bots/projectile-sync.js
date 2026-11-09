// Counts the movement packets a watcher gets for a projectile in flight over 40 server ticks.
// Vanilla only syncs projectiles through `ServerEntity.sendChanges`: every `updateInterval`
// ticks (arrows and tridents 20, thrown and hurting projectiles 10), motion only when it changed.
const { make } = require('./survival-lib')
const h = make('Watcher')
const { bot } = h
const X = 9900; const Y = 300; const Z = 0

const cases = [
  ['arrow', 'arrow', '{Motion:[0.5d,0d,0d]}'],
  ['trident', 'trident', '{Motion:[0.5d,0d,0d]}'],
  ['snowball', 'snowball', '{Motion:[0.5d,0d,0d]}'],
  ['fireball', 'fireball', '{Motion:[0.2d,0d,0d]}'],
  ['wind_charge', 'wind_charge', '{Motion:[0.5d,0d,0d]}']
]

let tracked = null
let counting = false
const counts = {}
bot._client.on('packet', (data, meta) => {
  if (counting && tracked !== null && data && data.entityId === tracked) {
    counts[meta.name] = (counts[meta.name] || 0) + 1
  }
})

h.start(async () => {
  h.cmd(`gamemode spectator ${bot.username}`)
  await h.tp(X + 0.5, Y, Z + 8.5)
  for (const [label, type, nbt] of cases) {
    h.cmd('kill @e[type=!player]')
    await h.sleep(500)
    tracked = null
    const onSpawn = (e) => { if (e.name === type) tracked = e.id }
    bot.on('entitySpawn', onSpawn)
    h.cmd(`summon ${type} ${X - 30.5} ${Y} ${Z + 0.5} ${nbt}`)
    await h.waitFor(() => tracked !== null, 5000)
    bot.removeListener('entitySpawn', onSpawn)
    // Time updates come every 20 ticks: count from one to the second one after it.
    const age0 = bot.time.age
    await h.waitFor(() => bot.time.age !== age0, 5000)
    for (const k of Object.keys(counts)) delete counts[k]
    const start = bot.time.age
    counting = true
    await h.waitFor(() => bot.time.age - start >= 40, 10000)
    counting = false
    h.result(`${label}.packets_40`, Object.keys(counts).sort().map(m => `${m}=${counts[m]}`).join(',') || 'none')
  }
  h.cmd('kill @e[type=!player]')
})
