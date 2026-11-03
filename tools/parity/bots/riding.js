// Riding: the server-side position of a player in a minecart and a boat, and of mobs riding
// other mobs, comes from the passenger and vehicle attachment points (vanilla positionRider).
const { make } = require('./survival-lib')
const h = make('Rider')
const { bot, Vec3 } = h
const X = 2100; const Y = 200; const Z = 0

const chat = []
bot.on('message', (m) => chat.push(m.toString()))

async function ask (cmd) {
  const c0 = chat.length
  bot.chat(cmd)
  if (!await h.waitFor(() => chat.length > c0, 5000)) return '<no reply>'
  await h.sleep(30)
  return chat[c0]
}

// `[1.5d, 201.5875d, 0.5d]` to `1.5,201.588,0.5`, so float noise doesn't count.
async function pos (selector) {
  const reply = await ask(`/data get entity ${selector} Pos`)
  const nums = reply.replace(/^.*data: /, '').match(/-?\d+(\.\d+)?(E-?\d+)?/g)
  return nums ? nums.map((n) => Number(Number(n).toFixed(3))).join(',') : reply
}

function nearest (name) {
  return bot.nearestEntity((e) => e.name === name)
}

async function ride (name, x, z) {
  h.cmd(`summon ${name} ${x} ${Y + 1} ${z} {Rotation:[90f,0f]}`)
  await h.waitFor(() => nearest(name), 5000)
  await h.sleep(20 * 50)
  // bot.mount writes the pre-26.1 use_entity layout; send the 26.3 interact packet directly.
  bot._client.write('use_entity', { target: nearest(name).id, hand: 0, location: { x: 0, y: 0.3, z: 0 }, sneaking: false })
  await h.waitFor(() => bot.vehicle, 5000)
  // mineflayer stops its physics ticks while riding, so wait on the clock.
  await h.sleep(1000)
  h.result(`${name}_riding`, !!bot.vehicle)
  h.result(`${name}_vehicle_pos`, await pos(`@e[type=${name},limit=1]`))
  h.result(`${name}_rider_pos`, await pos('@s'))
  bot._client.write('player_input', { inputs: { shift: true } })
  await h.waitFor(() => !bot.vehicle, 5000)
  bot._client.write('player_input', { inputs: {} })
  await h.sleep(1000)
  h.result(`${name}_dismounted`, !bot.vehicle)
  h.result(`${name}_dismount_pos`, await pos('@s'))
  h.cmd(`kill @e[type=${name}]`)
  h.cmd(`tp Rider ${X + 0.5} ${Y + 1} ${Z - 1.5}`)
  await h.sleep(1000)
}

h.start(async () => {
  await h.arena(X, Y, Z, 8)
  h.cmd(`tp Rider ${X + 0.5} ${Y + 1} ${Z - 1.5}`)
  await h.sleep(10 * 50)
  await ride('minecart', X + 0.5, Z + 0.5)
  await ride('oak_boat', X + 0.5, Z + 0.5)

  // Mob riders: a skeleton on a spider, a zombie on a chicken, a villager in a minecart.
  for (const [rider, vehicle] of [['skeleton', 'spider'], ['zombie', 'chicken'], ['villager', 'minecart']]) {
    h.cmd(`summon ${vehicle} ${X + 4.5} ${Y + 1} ${Z + 4.5} {NoAI:1b,Rotation:[90f,0f]}`)
    h.cmd(`summon ${rider} ${X + 4.5} ${Y + 1} ${Z + 4.5} {NoAI:1b}`)
    await h.sleep(10 * 50)
    h.result(`${rider}_mounts_${vehicle}`, await ask(`/ride @e[type=${rider},limit=1] mount @e[type=${vehicle},limit=1]`))
    await h.sleep(10 * 50)
    // Relative to the vehicle, which may get pushed around.
    const riderPos = (await pos(`@e[type=${rider},limit=1]`)).split(',').map(Number)
    const vehiclePos = (await pos(`@e[type=${vehicle},limit=1]`)).split(',').map(Number)
    h.result(`${rider}_on_${vehicle}`, riderPos.map((v, i) => Number((v - vehiclePos[i]).toFixed(3))).join(','))
    h.cmd(`kill @e[type=${rider}]`)
    h.cmd(`kill @e[type=${vehicle}]`)
    await h.sleep(10 * 50)
  }
})
