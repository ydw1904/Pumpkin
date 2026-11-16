// The way to and through the End: eyes of ender, frame rings (orientation, blocks inside),
// the portal sound, the obsidian platform and what arrives on it, the dragon fight (exit
// portal, egg, gateways in seed order, the respawn sequence with four crystals), end gateways
// (players, ridden vehicles), a cross-dimension /tp of a vehicle, the credits and the exit
// back to the spawn point, and the player boarding cooldown.
// pumpkin-settings: allow_end=true
const { make } = require('./survival-lib')
const h = make('Ender', 1500000)
const { bot, Vec3 } = h
const X = 7200; const Y = 200; const Z = 0
// Overworld landing pad for the spawn point and the cross-dimension /tp.
const PX = 7230
// Console commands run in the overworld; positions in the End need this prefix.
const END = 'execute in minecraft:the_end run '
const dimName = () => String(bot.game.dimension).replace('minecraft:', '')
const floorPos = (p) => `${Math.floor(p.x)},${Math.floor(p.y)},${Math.floor(p.z)}`

const chat = []
bot.on('message', (m) => chat.push(m.toString()))
async function ask (cmd) {
  const c0 = chat.length
  bot.chat(cmd)
  if (!await h.waitFor(() => chat.length > c0, 5000)) return '<no reply>'
  await h.sleep(30)
  return chat[c0]
}

let seat = null
bot._client.on('set_passengers', ({ entityId, passengers }) => {
  if (passengers.includes(bot.entity.id)) seat = entityId
  else if (seat === entityId) seat = null
})
const worldEvents = []
bot._client.on('world_event', (p) => worldEvents.push({ ...p, at: bot.entity ? bot.entity.position.clone() : null }))
const gameStates = []
bot._client.on('game_state_change', (p) => gameStates.push(String(p.reason)))
const wins = () => gameStates.filter((r) => r === '4' || r === 'win_game').length

const near = (name, x, y, z, r) => Object.values(bot.entities).filter((e) => e.name === name && e.position.distanceTo(new Vec3(x, y, z)) < r)
const nearest = (name, x, y, z) => near(name, x, y, z, 1e9).sort((a, b) => a.position.distanceTo(new Vec3(x, y, z)) - b.position.distanceTo(new Vec3(x, y, z)))[0]
const itemName = (e) => { try { const it = e.getDroppedItem(); return it ? `${it.name}x${it.count}` : '?' } catch (err) { return '?' } }
const beam = (e) => { if (!e || !e.metadata) return 'none'; const b = e.metadata[8]; return b ? `${b.x},${b.y},${b.z}` : 'none' }
const dedup = (arr) => arr.filter((v, i) => i === 0 || v !== arr[i - 1])

// The 12 frames around a 3x3 opening at (x..x+2, z..z+2), facing in or out.
function ring (x, z, inward) {
  const f = (a, b) => inward ? a : b
  const frames = []
  for (let i = 0; i < 3; i++) {
    frames.push([x + i, z - 1, f('south', 'north')], [x + i, z + 3, f('north', 'south')],
      [x - 1, z + i, f('east', 'west')], [x + 3, z + i, f('west', 'east')])
  }
  return frames
}
const portalCount = (x, y, z) => { let n = 0; for (let i = 0; i < 3; i++) for (let j = 0; j < 3; j++) if (h.blockName(x + i, y, z + j) === 'end_portal') n++; return n }

async function fillEyes (frames, cx, cz) {
  for (const [fx, fz] of frames) {
    const ax = Math.sign(fx - cx) * 1.5; const az = Math.sign(fz - cz) * 1.5
    await h.tp(fx + 0.5 + ax, Y + 1, fz + 0.5 + az)
    await bot.equip(bot.inventory.items().find(i => i.name === 'ender_eye'), 'hand')
    await bot.lookAt(new Vec3(fx + 0.5, Y + 1.8, fz + 0.5), true)
    await bot.activateBlock(h.block(fx, Y + 1, fz), new Vec3(0, 1, 0)).catch(e => h.result('eye_error', e.message))
    await h.ticks(4)
  }
}

async function throwEye () {
  await bot.equip(bot.inventory.items().find(i => i.name === 'ender_eye'), 'hand')
  await bot.look(0, Math.PI / 2, true)
  const before = h.inv().ender_eye || 0
  const start = bot.entity.position.clone()
  bot.activateItem()
  await h.sleep(1500)
  const eye = nearest('eye_of_ender', start.x, start.y, start.z)
  const used = before - (h.inv().ender_eye || 0)
  if (!eye) return { used, flight: 'none' }
  const d = eye.position.minus(start)
  const angle = Math.round(Math.atan2(d.z, d.x) * 180 / Math.PI / 5) * 5
  return { used, flight: `${angle}deg,${d.y > 0 ? 'up' : 'down'}` }
}

// Bedrock top of a column, scanning down.
function topOf (x, z, block, from, to) {
  for (let y = from; y >= to; y--) if (h.blockName(x, y, z) === block) return y
  return null
}

const GATEWAYS = []
for (let i = 0; i < 20; i++) {
  const a = 2.0 * (-Math.PI + (Math.PI / 20) * i)
  GATEWAYS.push([Math.floor(96 * Math.cos(a)), Math.floor(96 * Math.sin(a))])
}
async function gatewaysPresent () {
  const found = []
  for (let i = 0; i < 20; i++) {
    const [x, z] = GATEWAYS[i]
    const reply = await ask(`/execute if block ${x} 75 ${z} minecraft:end_gateway`)
    if (/passed/i.test(reply)) found.push(i)
  }
  return found
}

async function tpEnd (x, y, z) {
  h.cmd(`${END}tp Ender ${x} ${y} ${z} 0 0`)
  await h.waitFor(() => bot.entity.position.distanceTo(new Vec3(x, y, z)) < 1.5, 10000)
  await h.waitFor(h.chunksLoaded, 120000)
}

async function rideNew (tag, x, y, z) {
  h.cmd(`${END}summon pig ${x} ${y} ${z} {Tags:["${tag}"]}`)
  await h.waitFor(() => nearest('pig', x, y, z) && nearest('pig', x, y, z).position.distanceTo(new Vec3(x, y, z)) < 3, 5000)
  await h.sleep(500)
  h.cmd(`ride Ender mount @e[tag=${tag},limit=1]`)
  await h.waitFor(() => seat !== null, 5000)
  await h.sleep(500)
}

h.start(async () => {
  h.cmd(`forceload add ${PX - 8} -8 ${PX + 8} 8`)
  await h.arena(X + 6, Y, Z + 1, 12)
  await h.clear()
  h.cmd(`fill ${PX - 2} ${Y} -2 ${PX + 2} ${Y} 2 stone`)
  await h.give('ender_eye', 40)

  // Eyes of ender fly toward the nearest stronghold.
  await h.tp(X + 6.5, Y + 1, Z + 1.5)
  const thrown = await throwEye()
  h.result('eye_used', thrown.used)
  h.result('eye_flight', thrown.flight)

  // A ring whose frames all face out, and an inward one with stone in the opening.
  for (const [fx, fz, facing] of ring(X, Z, false)) h.cmd(`setblock ${fx} ${Y + 1} ${fz} end_portal_frame[facing=${facing}]`)
  for (const [fx, fz, facing] of ring(X + 8, Z, true)) h.cmd(`setblock ${fx} ${Y + 1} ${fz} end_portal_frame[facing=${facing}]`)
  h.cmd(`setblock ${X + 9} ${Y + 1} ${Z + 1} oak_planks`)
  await h.ticks(10)
  await fillEyes(ring(X, Z, false), X + 1, Z + 1)
  h.result('outward_ring_portal', portalCount(X, Y + 1, Z))
  const events0 = worldEvents.length
  await fillEyes(ring(X + 8, Z, true), X + 9, Z + 1)
  h.result('inward_ring_portal', portalCount(X + 8, Y + 1, Z))
  h.result('portal_sound', worldEvents.slice(events0).filter((e) => e.effectId === 1038)
    .map((e) => `${e.global}@${e.location.x - X},${e.location.y - Y},${e.location.z - Z}`).join(';') || 'none')
  await h.ticks(10)
  h.result('opening_stone_drop', near('item', X + 9.5, Y + 1, Z + 1.5, 4).map(itemName).sort().join(',') || 'none')
  h.cmd('kill @e[type=item]')

  // Blocks in the way of the End platform.
  h.cmd('execute in minecraft:the_end run forceload add 96 -4 104 4')
  await h.sleep(8000)
  let prepared = false
  for (let i = 0; i < 10 && !prepared; i++) {
    h.cmd('execute in minecraft:the_end run setblock 101 50 1 cobblestone')
    h.cmd('execute in minecraft:the_end run setblock 99 48 -1 end_stone')
    await h.sleep(1000)
    prepared = /passed/i.test(await ask('/execute in minecraft:the_end if block 99 48 -1 minecraft:end_stone'))
  }
  h.result('end_prepared', prepared)

  // A pig, an arrow and an item go through before the player.
  h.cmd(`summon pig ${X + 9.5} ${Y + 1.3} ${Z + 1.5} {NoAI:1b}`)
  h.cmd(`summon arrow ${X + 9.5} ${Y + 3} ${Z + 1.5} {Motion:[0.0,-0.5,0.0],pickup:0b}`)
  h.cmd(`summon item ${X + 9.5} ${Y + 1.3} ${Z + 1.5} {Item:{id:"minecraft:diamond",count:1},PickupDelay:32767}`)
  await h.sleep(3000)
  h.result('left_overworld', ['pig', 'arrow', 'item'].map((n) => near(n, X + 9.5, Y + 1, Z + 1.5, 6).length).join(','))
  h.cmd(`tp Ender ${X + 9.5} ${Y + 1.2} ${Z + 1.5} 0 30`)
  await h.waitFor(() => dimName() === 'the_end', 20000)
  await h.waitFor(h.chunksLoaded, 60000)
  await h.ticks(40)
  h.result('end_dim', dimName())
  h.result('end_pos', floorPos(bot.entity.position))
  h.result('end_rot', `${(bot.entity.yaw * 180 / Math.PI).toFixed(1)},${(bot.entity.pitch * 180 / Math.PI).toFixed(1)}`)
  h.result('end_arrivals', ['pig', 'arrow'].map((n) => near(n, 100.5, 50, 0.5, 5).length).join(','))
  h.result('end_items', near('item', 100.5, 50, 0.5, 6).map(itemName).sort().join(',') || 'none')
  const floor = []
  for (let dx = -2; dx <= 2; dx++) for (let dz = -2; dz <= 2; dz++) floor.push(h.blockName(100 + dx, 48, dz) === 'obsidian' ? 'o' : '.')
  h.result('end_platform', floor.join(''))
  h.result('end_cobble_cleared', h.blockName(101, 50, 1))
  const eyeInEnd = await throwEye()
  h.result('end_eye', `${eyeInEnd.used},${eyeInEnd.flight}`)
  h.cmd('kill @e[type=item]')
  h.cmd('kill @e[type=pig]')
  h.cmd('kill @e[type=arrow]')

  // A global sound from the overworld reaches a player in the End at their own position.
  h.cmd('forceload add 7240 16 7250 26')
  await h.sleep(2000)
  const events1 = worldEvents.length
  h.cmd('summon wither 7245 230 20 {Invul:3}')
  await h.waitFor(() => worldEvents.slice(events1).some((e) => e.effectId === 1023), 8000)
  h.result('wither_sound', worldEvents.slice(events1).filter((e) => e.effectId === 1023)
    .map((e) => `${e.global}@${e.at && Math.abs(e.location.x - Math.floor(e.at.x)) <= 1 && Math.abs(e.location.z - Math.floor(e.at.z)) <= 1 ? 'player' : `${e.location.x},${e.location.y},${e.location.z}`}`).join(';') || 'none')
  h.cmd('kill @e[type=wither]')
  h.cmd('forceload remove 7240 16 7250 26')

  // The dragon fight.
  h.cmd('gamemode creative Ender')
  await h.waitFor(() => bot.game.gameMode === 'creative', 5000)
  h.cmd(`${END}tp Ender 6.5 90 6.5`)
  await h.waitFor(() => bot.entity.position.distanceTo(new Vec3(6.5, 90, 6.5)) < 2, 10000)
  bot.creative.startFlying()
  await h.waitFor(h.chunksLoaded, 120000)
  h.result('dragon_spawned', await h.waitFor(() => nearest('ender_dragon', 0, 100, 0), 30000))
  await h.sleep(2000)
  const pillarTop = topOf(0, 0, 'bedrock', 100, 30)
  const oy = pillarTop === null ? 64 : pillarTop - 3
  h.result('podium_y', oy)
  h.result('exit_portal_before', portalCount(-1, oy, -1))
  const spikeTop = topOf(42, 0, 'bedrock', 130, 60)
  h.result('spike0_top', spikeTop)

  // A block left on the podium drops when the exit portal opens.
  h.cmd(`${END}setblock 3 ${oy + 1} 0 cobblestone`)
  h.cmd('kill @e[type=item]')
  await h.sleep(500)
  h.cmd('kill @e[type=ender_dragon]')
  await h.sleep(3000)
  h.result('exit_portal_kill1', portalCount(-1, oy, -1))
  h.result('egg_kill1', topOf(0, 0, 'dragon_egg', oy + 12, oy))
  h.result('podium_items', near('item', 0.5, oy + 1, 0.5, 10).map(itemName).sort().join(',') || 'none')
  const gw1 = await gatewaysPresent()
  h.result('gateways_kill1', gw1.join(','))

  // Respawn the dragon with four crystals on the exit portal; the spike is rebuilt.
  if (spikeTop !== null) h.cmd(`${END}setblock 42 ${spikeTop - 3} 0 air`)
  await h.give('end_crystal', 4)
  const place = async (x, z) => {
    for (let i = 0; i < 3 && near('end_crystal', x + 0.5, oy + 1, z + 0.5, 1).length === 0; i++) {
      const stack = bot.inventory.items().find(it => it.name === 'end_crystal')
      if (!stack) break
      await bot.equip(stack, 'hand')
      await bot.lookAt(new Vec3(x + 0.5, oy + 1, z + 0.5), true)
      await bot.activateBlock(h.block(x, oy, z), new Vec3(0, 1, 0)).catch(e => h.result('crystal_error', e.message))
      await h.sleep(500)
    }
  }
  // Each from the rim block next to it.
  await tpEnd(3.5, oy + 1, 1.5)
  await place(3, 0)
  await tpEnd(1.5, oy + 1, 3.5)
  await place(0, 3)
  await tpEnd(-2.5, oy + 1, 1.5)
  await place(-3, 0)
  h.result('crystals_before_last', near('end_crystal', 0.5, oy + 1, 0.5, 5).length)
  await tpEnd(1.5, oy + 1, -2.5)
  await place(0, -3)
  const t0 = bot.time.age
  h.cmd(`${END}tp Ender 10.5 ${oy + 6} 10.5`)
  const crystalBeams = []; const spikeBeams = []; let closedAt = null; let seenAt = null
  const done = await h.waitFor(() => {
    crystalBeams.push(beam(nearest('end_crystal', 3.5, oy + 1, 0.5)))
    if (spikeTop !== null) spikeBeams.push(beam(nearest('end_crystal', 42.5, spikeTop + 1, 0.5)))
    if (closedAt === null && portalCount(-1, oy, -1) === 0) closedAt = bot.time.age - t0
    if (seenAt === null && Object.values(bot.entities).some((e) => e.name === 'ender_dragon')) seenAt = Date.now()
    return seenAt !== null && Date.now() - seenAt > 3000
  }, 95000)
  h.result('respawn_dragon', done)
  h.result('respawn_portal_closed', closedAt !== null && closedAt < 100)
  h.result('respawn_crystal_beams', dedup(crystalBeams).join(' > '))
  h.result('respawn_spike_beams', dedup(spikeBeams).join(' > '))
  await h.sleep(3000)
  h.result('respawn_crystals_left', near('end_crystal', 0.5, oy + 1, 0.5, 5).length)
  h.result('respawn_exit_portal', portalCount(-1, oy, -1))
  if (spikeTop !== null) h.result('respawn_spike_rebuilt', h.blockName(42, spikeTop - 3, 0))

  h.cmd('kill @e[type=ender_dragon]')
  await h.sleep(3000)
  h.result('exit_portal_kill2', portalCount(-1, oy, -1))
  let eggs = 0
  for (let y = oy; y <= oy + 12; y++) if (h.blockName(0, y, 0) === 'dragon_egg') eggs++
  h.result('eggs_kill2', eggs)
  const gw2 = await gatewaysPresent()
  h.result('gateways_kill2', gw2.join(','))

  // A player through the first gateway, then a ridden pig through the second.
  const first = gw1.length ? GATEWAYS[gw1[0]] : null
  const second = gw2.find((i) => !gw1.includes(i))
  const secondPos = second !== undefined ? GATEWAYS[second] : null
  if (first) {
    h.cmd(`${END}tp Ender ${first[0] + 0.5} 75 ${first[1] + 0.5}`)
    const out = await h.waitFor(() => bot.entity.position.distanceTo(new Vec3(0, 75, 0)) > 400, 20000)
    await h.waitFor(h.chunksLoaded, 60000)
    await h.sleep(2000)
    h.result('gateway_travel', out)
    h.result('gateway_arrival', floorPos(bot.entity.position))
    const a = bot.entity.position.floored()
    let exit = 'none'
    h.result('gateway_floor', [1, 2, 3].map((d) => h.blockName(a.x, a.y - d, a.z)).join(','))
    for (let dx = -16; dx <= 16 && exit === 'none'; dx++) for (let dz = -16; dz <= 16 && exit === 'none'; dz++) for (let dy = -30; dy <= 30; dy++) if (h.blockName(a.x + dx, a.y + dy, a.z + dz) === 'end_gateway') { exit = `${dx},${dy},${dz}`; break }
    h.result('gateway_exit_offset', exit)
  }
  if (secondPos) {
    h.cmd(`${END}tp Ender ${secondPos[0] + 4.5} 76 ${secondPos[1] + 0.5}`)
    await h.waitFor(() => bot.entity.position.distanceTo(new Vec3(secondPos[0] + 4.5, 76, secondPos[1] + 0.5)) < 2, 20000)
    await h.waitFor(h.chunksLoaded, 60000)
    await rideNew('gp', secondPos[0] + 4.5, 75, secondPos[1] + 0.5)
    h.result('gateway_ride_mounted', seat !== null)
    h.cmd(`tp @e[tag=gp,limit=1] ${secondPos[0] + 0.5} 75 ${secondPos[1] + 0.5}`)
    const out = await h.waitFor(() => bot.entity.position.distanceTo(new Vec3(0, 75, 0)) > 400, 10000)
    await h.sleep(2000)
    h.result('gateway_ride_travel', out)
    h.result('gateway_ride_seat', seat !== null)
  }
  h.cmd('ride Ender dismount')
  h.cmd('kill @e[type=pig]')
  await h.sleep(1000)

  // /tp of a ridden pig to the overworld takes its rider along.
  h.cmd(`${END}tp Ender 6.5 90 6.5`)
  await h.waitFor(() => bot.entity.position.distanceTo(new Vec3(6.5, 90, 6.5)) < 2, 20000)
  await h.waitFor(h.chunksLoaded, 60000)
  await rideNew('gq', 8.5, 90, 6.5)
  h.result('tp_ride_mounted', seat !== null)
  h.cmd(`execute in minecraft:overworld run tp @e[tag=gq,limit=1] ${PX + 0.5} ${Y + 1} 0.5`)
  await h.waitFor(() => dimName() === 'overworld', 15000)
  await h.sleep(3000)
  h.result('tp_ride_dim', dimName())
  h.result('tp_ride_pos', floorPos(bot.entity.position))
  h.result('tp_ride_seat', seat !== null)
  h.cmd('ride Ender dismount')
  h.cmd('kill @e[type=pig]')
  await h.sleep(1000)

  // The exit portal shows the credits once, then sends the player to their spawn point.
  h.cmd(`spawnpoint Ender ${PX} ${Y + 1} 0`)
  h.cmd(`execute in minecraft:the_end run tp Ender 1.5 ${oy} 0.5`)
  await h.waitFor(() => wins() > 0, 15000)
  await h.sleep(1000)
  h.result('credits_first', wins())
  h.result('credits_dim', dimName())
  bot._client.write('client_command', { actionId: 'perform_respawn' })
  await h.waitFor(() => dimName() === 'overworld', 15000)
  await h.waitFor(h.chunksLoaded, 60000)
  await h.sleep(1000)
  h.result('credits_return', `${dimName()} ${floorPos(bot.entity.position)}`)
  h.result('seen_credits', (await ask('/data get entity Ender seenCredits')).replace(/^.*data: /, ''))
  h.cmd(`execute in minecraft:the_end run tp Ender 1.5 ${oy} 0.5`)
  await h.waitFor(() => dimName() === 'the_end', 15000)
  await h.waitFor(() => dimName() === 'overworld', 15000)
  await h.waitFor(h.chunksLoaded, 60000)
  await h.sleep(1000)
  h.result('credits_second', wins())
  h.result('exit_second', `${dimName()} ${floorPos(bot.entity.position)}`)

  // A mob through the exit portal lands on top of the world spawn column.
  h.cmd(`setworldspawn ${PX} ${Y - 5} 0`)
  h.cmd('kill @e[type=pig]')
  h.cmd(`${END}forceload add 0 0`)
  await h.sleep(3000)
  h.cmd(`${END}summon pig 1.5 ${oy + 0.5} 0.5 {NoAI:1b,Tags:["exitpig"]}`)
  await h.sleep(3000)
  h.result('exit_pig', (await ask('/data get entity @e[tag=exitpig,limit=1] Pos')).replace(/^.*data: /, '').replace(/(\.\d)\d*d/g, '$1'))

  // Getting off a boat leaves a player free to board again at once.
  h.cmd('gamemode survival Ender')
  h.cmd(`tp Ender ${PX + 0.5} ${Y + 1} -1.5`)
  await h.sleep(1000)
  h.cmd(`summon oak_boat ${PX + 0.5} ${Y + 1} 1.5`)
  await h.waitFor(() => nearest('oak_boat', PX + 0.5, Y + 1, 1.5), 5000)
  await h.sleep(1000)
  const boat = nearest('oak_boat', PX + 0.5, Y + 1, 1.5)
  if (boat) {
    bot._client.write('use_entity', { target: boat.id, hand: 0, location: { x: 0, y: 0.3, z: 0 }, sneaking: false })
    h.result('boat_mount', await h.waitFor(() => seat !== null, 5000))
    h.cmd('ride Ender dismount')
    await h.waitFor(() => seat === null, 5000)
    bot._client.write('use_entity', { target: boat.id, hand: 0, location: { x: 0, y: 0.3, z: 0 }, sneaking: false })
    h.result('boat_remount', await h.waitFor(() => seat !== null, 2000))
  }
})
