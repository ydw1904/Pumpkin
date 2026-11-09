// Hurting projectiles (fireballs, wither skulls, wind charges) speed up along their flight by
// their acceleration power and keep `inertia` of their speed each tick, without gravity or the
// drag of thrown items. Records how far each flies in open air and through still water, where
// they keep less of their speed (wind charges keep all of it, snowballs get water drag).
const { make } = require('./survival-lib')
const h = make('Watcher')
const { bot } = h
const X = 9900; const Y = 240; const Z = 0
const WATER_Y = Y - 40

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
async function gameTime () {
  const m = (await ask('/time query gametime')).match(/(-?\d+)/)
  return m ? Number(m[1]) : null
}
// Runs `/tick step` and returns the reply to `cmd` once the step is over, read until two
// readings agree. A slow server (a debug build still loading the area) can take well over a
// second per tick, so a fixed wait isn't enough.
async function afterStep (steps, cmd) {
  const start = await gameTime()
  await ask(`/tick step ${steps}`)
  const end = Date.now() + 60000
  while (Date.now() < end && (await gameTime()) < start + steps) await h.sleep(250)
  let last = null
  for (let i = 0; i < 20; i++) {
    const reply = await ask(cmd)
    if (reply === last) break
    last = reply
    await h.sleep(250)
  }
  return last
}
const number = (reply) => {
  const m = reply.match(/(-?\d+(?:\.\d+)?(?:E-?\d+)?)d?\s*$/)
  return m ? Number(m[1]) : null
}

const cases = [
  ['fireball', 'fireball', ''],
  ['small_fireball', 'small_fireball', ''],
  ['dragon_fireball', 'dragon_fireball', ''],
  ['wither_skull', 'wither_skull', ''],
  ['dangerous_wither_skull', 'wither_skull', ',dangerous:1b'],
  ['wind_charge', 'wind_charge', '']
]
const waterCases = [
  ['fireball', 'fireball', '0.2d,0d,0d', 0],
  ['wither_skull', 'wither_skull', '0.2d,0d,0d', 0],
  ['wind_charge', 'wind_charge', '0.2d,0d,0d', 0],
  ['snowball', 'snowball', '1.0d,0.2d,0d', 1]
]

h.start(async () => {
  await h.arena(X, WATER_Y, Z, 4)
  h.cmd(`fill ${X - 2} ${WATER_Y + 1} ${Z - 2} ${X + 20} ${WATER_Y + 5} ${Z + 2} stone`)
  h.cmd(`fill ${X - 1} ${WATER_Y + 1} ${Z - 1} ${X + 19} ${WATER_Y + 4} ${Z + 1} water`)
  h.cmd(`gamemode spectator ${bot.username}`)
  h.cmd(`tp ${bot.username} ${X + 8.5} ${Y} ${Z + 6.5}`)
  await h.ticks(20)
  // Frozen, so each projectile flies exactly the stepped ticks however fast the server runs.
  h.cmd('tick freeze')
  for (const [label, type, nbt] of cases) {
    h.cmd('kill @e[type=!player]')
    await h.sleep(500)
    h.cmd(`summon ${type} ${X + 0.5} ${Y} ${Z + 0.5} {Motion:[0.2d,0d,0d]${nbt}}`)
    await h.sleep(500)
    // The server's own position after 20 ticks, to a tenth of a block.
    const reply = await afterStep(20, `/data get entity @e[type=${type},limit=1] Pos[0]`)
    const x = number(reply)
    h.result(`${label}.x_after_20`, x === null ? reply.slice(0, 60) : Math.round((x - X - 0.5) * 10) / 10)
  }
  for (const [label, type, motion, axis] of waterCases) {
    h.cmd('kill @e[type=!player]')
    await h.sleep(500)
    const from = [X + 0.5, WATER_Y + 2.5, Z + 0.5]
    h.cmd(`summon ${type} ${from.join(' ')} {Motion:[${motion}]}`)
    await h.sleep(500)
    const reply = await afterStep(10, `/data get entity @e[type=${type},limit=1] Pos[${axis}]`)
    const p = number(reply)
    h.result(`water.${label}.moved_10`, p === null ? reply.slice(0, 60) : Math.round((p - from[axis]) * 100) / 100)
    const v = number(await ask(`/data get entity @e[type=${type},limit=1] Motion[0]`))
    h.result(`water.${label}.vx_10`, v === null ? 'none' : Math.round(v * 1000) / 1000)
  }
  // Vanilla `AbstractWindCharge.canHitEntity`: a wind charge flies through an end crystal.
  h.cmd('kill @e[type=!player]')
  await h.sleep(500)
  h.cmd(`summon end_crystal ${X + 6.5} ${Y} ${Z + 0.5} {ShowBottom:0b}`)
  h.cmd(`summon wind_charge ${X + 0.5} ${Y + 0.5} ${Z + 0.5} {Motion:[0.5d,0d,0d]}`)
  await h.sleep(500)
  h.result('crystal.wind_charge_left', await afterStep(30, '/execute if entity @e[type=wind_charge]'))
  h.result('crystal.crystal_left', await ask('/execute if entity @e[type=end_crystal]'))
  h.cmd('tick unfreeze')
  h.cmd('kill @e[type=!player]')
})
