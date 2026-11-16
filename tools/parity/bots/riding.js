// Riding: the server-side position of a player in a minecart and a boat, and of mobs riding
// other mobs, comes from the passenger and vehicle attachment points (vanilla positionRider).
// Then how a riding player gets off: mineflayer's dismount (a jump input), sneaking, /ride
// dismount, a /tp of the rider (vanilla removeVehicle first) or of the vehicle (the rider
// comes along), and an ender pearl.
const { make } = require('./survival-lib')
const h = make('Rider')
const { bot } = h
const X = 2100; const Y = 200; const Z = 0

const chat = []
bot.on('message', (m) => chat.push(m.toString()))

// mineflayer only clears bot.vehicle when a passenger list names the bot, which a dismount's
// never does, so follow the passenger lists here instead.
let seat = null
bot._client.on('set_passengers', ({ entityId, passengers }) => {
  if (passengers.includes(bot.entity.id)) seat = entityId
  else if (seat === entityId) seat = null
})

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

// Whether the bot still gets physics ticks: mineflayer pauses them on mount and resumes them
// on the next position packet.
async function ticking () {
  try {
    await bot.waitForTicks(5)
    return true
  } catch (e) {
    return false
  }
}

async function summon (name, nbt) {
  h.cmd(`summon ${name} ${X + 0.5} ${Y + 1} ${Z + 0.5} {Rotation:[90f,0f]${nbt}}`)
  await h.waitFor(() => nearest(name), 5000)
  await h.sleep(20 * 50)
}

async function reset (name) {
  if (seat !== null) h.cmd('ride Rider dismount')
  bot._client.write('player_input', { inputs: {} })
  h.cmd(`kill @e[type=${name}]`)
  h.cmd(`tp Rider ${X + 0.5} ${Y + 1} ${Z - 1.5}`)
  await h.sleep(1000)
}

// /ride mounts with force, so the boarding cooldown of the previous scene doesn't matter.
async function rideCommand (name, nbt) {
  await summon(name, nbt)
  h.cmd(`ride Rider mount @e[type=${name},limit=1]`)
  await h.waitFor(() => seat !== null, 5000)
  await h.sleep(1000)
}

async function ride (name, nbt = '') {
  await summon(name, nbt)
  // bot.mount writes the pre-26.1 use_entity layout; send the 26.3 interact packet directly.
  bot._client.write('use_entity', { target: nearest(name).id, hand: 0, location: { x: 0, y: 0.3, z: 0 }, sneaking: false })
  await h.waitFor(() => seat !== null, 5000)
  // mineflayer stops its physics ticks while riding, so wait on the clock.
  await h.sleep(1000)
  h.result(`${name}_riding`, seat !== null)
  h.result(`${name}_vehicle_pos`, await pos(`@e[type=${name},limit=1]`))
  h.result(`${name}_rider_pos`, await pos('@s'))
  // mineflayer's dismount presses jump, which only sneaking does in vanilla.
  bot.dismount()
  await h.sleep(1000)
  bot._client.write('player_input', { inputs: {} })
  h.result(`${name}_seated_after_jump`, seat !== null)
  bot._client.write('player_input', { inputs: { shift: true } })
  await h.waitFor(() => seat === null, 5000)
  bot._client.write('player_input', { inputs: {} })
  await h.sleep(1000)
  h.result(`${name}_dismounted`, seat === null)
  h.result(`${name}_dismount_pos`, await pos('@s'))
  await reset(name)

  await rideCommand(name, nbt)
  h.result(`${name}_ride_command_seated`, seat !== null)
  h.result(`${name}_ride_dismount`, await ask('/ride Rider dismount'))
  await h.sleep(1000)
  h.result(`${name}_ride_dismount_seated`, seat !== null)
  h.result(`${name}_ride_dismount_pos`, await pos('@s'))
  await reset(name)

  await rideCommand(name, nbt)
  h.cmd(`tp Rider ${X + 3.5} ${Y + 1} ${Z + 2.5}`)
  await h.sleep(1000)
  h.result(`${name}_tp_rider_seated`, seat !== null)
  h.result(`${name}_tp_rider_ticking`, await ticking())
  h.result(`${name}_tp_rider_pos`, await pos('@s'))
  h.result(`${name}_tp_rider_vehicle_pos`, await pos(`@e[type=${name},limit=1]`))
  await reset(name)

  await rideCommand(name, nbt)
  h.cmd(`tp @e[type=${name},limit=1] ${X - 2.5} ${Y + 1} ${Z + 2.5}`)
  await h.sleep(1000)
  h.result(`${name}_tp_vehicle_seated`, seat !== null)
  h.result(`${name}_tp_vehicle_pos`, await pos(`@e[type=${name},limit=1]`))
  h.result(`${name}_tp_vehicle_rider_pos`, await pos('@s'))
  // Where the client was sent: the passenger gets its own position packet. Only x and z, since
  // mineflayer resumes falling physics on that packet while it still thinks it rides.
  const { x, z } = bot.entity.position
  h.result(`${name}_tp_vehicle_client_xz`, `${x.toFixed(1)},${z.toFixed(1)}`)
  await reset(name)
}

// An ender pearl teleports its thrower off the vehicle. Summoned with the bot as owner, so the
// random spread of a throw stays out of it.
function uuidInts (uuid) {
  const hex = uuid.replace(/-/g, '')
  return [0, 8, 16, 24].map((i) => parseInt(hex.slice(i, i + 8), 16) | 0).join(',')
}

async function pearl () {
  await rideCommand('minecart', '')
  h.cmd(`summon ender_pearl ${X + 3.5} ${Y + 2} ${Z + 0.5} {Motion:[0.0,-0.5,0.0],Owner:[I;${uuidInts(bot.player.uuid)}]}`)
  await h.sleep(2000)
  h.result('pearl_seated', seat !== null)
  h.result('pearl_rider_pos', await pos('@s'))
  await reset('minecart')
}

h.start(async () => {
  await h.arena(X, Y, Z, 8)
  h.cmd(`tp Rider ${X + 0.5} ${Y + 1} ${Z - 1.5}`)
  await h.sleep(10 * 50)
  await ride('minecart')
  await ride('oak_boat')
  await ride('horse', ',Tame:1b,NoAI:1b')
  await pearl()

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
