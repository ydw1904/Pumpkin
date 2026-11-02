// Two players and beds: the action bar sleep status, spectators not counting, skipping the night.
const mineflayer = require('mineflayer')
const { make } = require('./survival-lib')
const h = make('SleeperA')
const { bot, Vec3 } = h
const X = 1700; const Y = 200; const Z = 0

const describe = (j) => j.translate ? j.translate + (j.with ? '(' + j.with.map(w => typeof w === 'object' ? (w.text ?? w.translate) : w).join(',') + ')' : '') : null
const msgsA = []; const msgsB = []
const listen = (b, into) => b.on('message', (j) => { const s = describe(j); if (s && s.startsWith('sleep')) into.push(s) })
const take = (m) => { const s = m.join('|') || 'none'; m.length = 0; return s }
listen(bot, msgsA)

async function placeBed (b, x, z) {
  h.cmd(`give ${b.username} white_bed`)
  await h.waitFor(() => b.inventory.items().some(i => i.name === 'white_bed'), 5000)
  await b.equip(b.inventory.items().find(i => i.name === 'white_bed'), 'hand')
  await b.lookAt(new Vec3(x + 0.5, Y + 1, z + 0.5), true)
  await b.placeBlock(b.blockAt(new Vec3(x, Y, z)), new Vec3(0, 1, 0)).catch(() => {})
  await h.ticks(5)
  const bed = b.blockAt(new Vec3(x, Y + 1, z))
  return bed && bed.name === 'white_bed' ? bed : null
}

h.start(async () => {
  await h.arena(X, Y, Z, 10)
  const b2 = mineflayer.createBot({ host: '127.0.0.1', port: Number(process.env.PORT), username: 'SleeperB', version: '26.3', auth: 'offline' })
  listen(b2, msgsB)
  await new Promise((resolve) => b2.once('spawn', resolve))
  h.cmd(`tp SleeperB ${X + 0.5} ${Y + 1} ${Z + 4.5}`)
  h.cmd('gamemode survival SleeperB')
  await h.waitFor(() => b2.entity.position.distanceTo(new Vec3(X + 0.5, Y + 1, Z + 4.5)) < 1.5, 20000)
  await h.ticks(20)
  const bedA = await placeBed(bot, X + 2, Z)
  const bedB = await placeBed(b2, X + 2, Z + 4)
  h.result('beds', `${!!bedA},${!!bedB}`)
  h.cmd('time set 18000')
  await h.ticks(10)
  take(msgsA); take(msgsB)

  await bot.sleep(bedA).catch(e => h.result('a_sleep_error', e.message))
  await h.ticks(20)
  h.result('one_asleep_a', take(msgsA))
  h.result('one_asleep_b', take(msgsB))

  h.cmd('gamemode spectator SleeperB')
  await h.ticks(20)
  h.result('other_spectating', take(msgsA))
  await h.ticks(120)
  h.result('night_skipped', bot.time.timeOfDay < 1000)
  b2.quit()
})
