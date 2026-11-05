// Hurting projectiles (fireballs, wither skulls, wind charges) speed up along their flight by
// their acceleration power and keep `inertia` of their speed each tick, without gravity or the
// drag of thrown items. Records how far each flies in open air.
const { make } = require('./survival-lib')
const h = make('Watcher')
const { bot } = h
const X = 9900; const Y = 240; const Z = 0

const chat = []
bot.on('message', (m) => chat.push(m.toString()))
// Runs a command as the bot and returns its first reply line.
async function ask (cmd) {
  const c0 = chat.length
  bot.chat(cmd)
  if (!await h.waitFor(() => chat.length > c0, 5000)) return '<no reply>'
  await h.sleep(50)
  return chat[c0]
}

const cases = [
  ['fireball', 'fireball', ''],
  ['small_fireball', 'small_fireball', ''],
  ['dragon_fireball', 'dragon_fireball', ''],
  ['wither_skull', 'wither_skull', ''],
  ['dangerous_wither_skull', 'wither_skull', ',dangerous:1b'],
  ['wind_charge', 'wind_charge', '']
]

h.start(async () => {
  await h.arena(X, Y - 40, Z, 4)
  h.cmd(`gamemode spectator ${bot.username}`)
  h.cmd(`tp ${bot.username} ${X + 8.5} ${Y} ${Z + 6.5}`)
  await h.ticks(20)
  // Frozen, so each projectile flies exactly 20 server ticks however fast the server runs.
  h.cmd('tick freeze')
  for (const [label, type, nbt] of cases) {
    h.cmd('kill @e[type=!player]')
    await h.sleep(500)
    h.cmd(`summon ${type} ${X + 0.5} ${Y} ${Z + 0.5} {Motion:[0.2d,0d,0d]${nbt}}`)
    await h.sleep(500)
    await ask('/tick step 20')
    await h.sleep(3000)
    // The server's own position after 20 ticks, to a tenth of a block.
    const reply = await ask(`/data get entity @e[type=${type},limit=1] Pos[0]`)
    const m = reply.match(/(-?\d+(?:\.\d+)?)d?\s*$/)
    h.result(`${label}.x_after_20`, m ? Math.round((Number(m[1]) - X - 0.5) * 10) / 10 : reply.slice(0, 60))
  }
  h.cmd('tick unfreeze')
  h.cmd('kill @e[type=!player]')
})
