// Passenger changes seen by a player who isn't riding: vanilla sends them only from the vehicle's
// tracker (`ServerEntity.sendChanges`), so a watcher gets one set_passengers per mount or dismount,
// for a player rider and for a mob rider alike.
const mineflayer = require('mineflayer')
const { make } = require('./survival-lib')
const h = make('Watcher')
const { bot } = h
const X = 3700; const Y = 200; const Z = 0

const packets = {}
bot._client.on('set_passengers', (p) => {
  packets[p.entityId] = (packets[p.entityId] || 0) + 1
})
const nearest = (name) => bot.nearestEntity((e) => e.name === name)
const take = (id) => { const n = packets[id] || 0; packets[id] = 0; return n }

h.start(async () => {
  await h.arena(X, Y, Z, 6)
  const rider = mineflayer.createBot({ host: '127.0.0.1', port: Number(process.env.PORT), username: 'Rider', version: '26.3', auth: 'offline' })
  await new Promise((resolve) => rider.once('spawn', resolve))
  h.cmd(`tp Rider ${X + 2.5} ${Y + 1} ${Z + 0.5}`)
  h.cmd(`summon pig ${X + 2.5} ${Y + 1} ${Z + 2.5} {NoAI:1b,PersistenceRequired:1b,Tags:["vehicle"]}`)
  h.cmd(`summon zombie ${X - 2.5} ${Y + 1} ${Z + 0.5} {NoAI:1b,PersistenceRequired:1b}`)
  h.cmd(`summon pig ${X - 2.5} ${Y + 1} ${Z + 2.5} {NoAI:1b,PersistenceRequired:1b,Tags:["mobvehicle"]}`)
  await h.waitFor(() => nearest('zombie') && Object.values(bot.entities).filter(e => e.name === 'pig').length === 2, 10000)
  await h.ticks(20)
  const pigs = Object.values(bot.entities).filter(e => e.name === 'pig')
  const pig = pigs.find(e => e.position.x > X)
  const mobPig = pigs.find(e => e.position.x < X)
  take(pig.id); take(mobPig.id)

  h.cmd('ride Rider mount @e[type=pig,tag=vehicle,limit=1]')
  await h.ticks(40)
  h.result('player_mount_packets', take(pig.id))
  h.cmd('ride Rider dismount')
  await h.ticks(40)
  h.result('player_dismount_packets', take(pig.id))

  h.cmd(`ride @e[type=zombie,limit=1] mount @e[type=pig,tag=mobvehicle,limit=1]`)
  await h.ticks(40)
  h.result('mob_mount_packets', take(mobPig.id))
  h.cmd('ride @e[type=zombie,limit=1] dismount')
  await h.ticks(40)
  h.result('mob_dismount_packets', take(mobPig.id))
  rider.quit()
})
