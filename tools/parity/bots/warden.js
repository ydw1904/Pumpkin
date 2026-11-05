// Warden anger as a client sees it: a warden next to a still player sniffs it, gets angrier with
// every touch, roars and attacks. Lists the poses, the synced anger level, entity events and
// sounds along the way.
const { make } = require('./survival-lib')
const h = make('Suspect', 600000)
const { bot } = h
const X = 3200; const Y = 200; const Z = 0
const POSE = 6
const CLIENT_ANGER_LEVEL = 16
const SNIFFING = 12
const ROARING = 11
const ATTACK = 4

let warden = null
const poses = new Set()
const statuses = new Set()
const heard = new Set()
bot._client.on('entity_status', (p) => {
  if (warden && p.entityId === warden.id) statuses.add(p.entityStatus)
})
bot._client.on('sound_effect', (p) => {
  const s = p.sound
  const id = s ? s.soundId : p.soundId
  if (id >= WARDEN_SOUNDS[0] && id <= WARDEN_SOUNDS[1] && !AMBIENT_SOUNDS.includes(id)) heard.add(id)
})
bot.on('physicsTick', () => {
  if (warden && warden.metadata[POSE] !== undefined) poses.add(warden.metadata[POSE])
})
const anger = () => (warden && warden.metadata[CLIENT_ANGER_LEVEL]) || 0
// The raw ids of the `entity.warden.*` sounds, which both servers share, without the ambient
// agitated, ambient and angry sounds, which play at random.
const WARDEN_SOUNDS = [1739, 1758]
const AMBIENT_SOUNDS = [1739, 1740, 1741]
const set = (s) => [...s].sort((a, b) => a - b).join(',') || 'none'

h.start(async () => {
  await h.arena(X, Y, Z, 8)
  h.cmd('time set noon')
  h.cmd('gamerule advance_time false')
  h.cmd('difficulty normal')
  h.cmd(`gamemode survival ${bot.username}`)
  h.cmd(`effect give ${bot.username} resistance infinite 4 true`)
  h.cmd(`effect give ${bot.username} regeneration infinite 4 true`)
  await h.tp(X + 0.5, Y + 1, Z + 0.5)
  await h.ticks(20)

  h.cmd('kill @e[type=!player]')
  await h.ticks(20)
  // No NBT, so the summon finalizes the spawn and sets the dig cooldown.
  h.cmd(`summon warden ${X + 0.5} ${Y + 1} ${Z + 4.5}`)
  await h.waitFor(() => (warden = Object.values(bot.entities).find(e => e.name === 'warden')), 5000)
  h.result('summoned', !!warden)
  if (!warden) process.exit(0)

  h.result('sniffed', await h.waitFor(() => warden.metadata[POSE] === SNIFFING, 40000))
  h.result('angered_by_sniff', await h.waitFor(() => anger() > 0, 15000))
  h.result('sniff_anger_10s', Math.floor(anger() / 10) * 10)
  h.result('roared', await h.waitFor(() => warden.metadata[POSE] === ROARING, 60000))
  h.result('attacked', await h.waitFor(() => statuses.has(ATTACK), 20000))
  h.result('angry_after_attack', anger() >= 80)
  h.result('poses', set(poses))
  h.result('statuses', set(statuses))
  h.result('sounds', set(heard))

  // A creative player can't be targeted, so the warden drops it and calms down.
  h.cmd(`gamemode creative ${bot.username}`)
  h.result('calmed', await h.waitFor(() => anger() < 80, 15000))

  // Darkness every 6 seconds reaches survival players, but not the warden's allies.
  h.cmd('team add allies')
  h.cmd(`team join allies ${bot.username}`)
  h.cmd('team join allies @e[type=warden]')
  h.cmd(`effect clear ${bot.username} darkness`)
  h.cmd(`gamemode survival ${bot.username}`)
  await h.ticks(10)
  effects.clear()
  await h.serverTicks(140)
  h.result('darkness.allied', set(effects))
  h.cmd('team leave @e[type=warden]')
  await h.serverTicks(140)
  h.result('darkness.not_allied', set(effects))
  h.cmd(`gamemode creative ${bot.username}`)
  h.cmd('kill @e[type=warden]')
  await h.ticks(20)

  // A boat takes pushable mobs that touch it, but a warden can't ride.
  h.cmd(`summon oak_boat ${X + 0.5} ${Y + 1} ${Z + 4.5}`)
  h.cmd(`summon warden ${X + 0.5} ${Y + 1} ${Z + 4.5} {NoAI:1b}`)
  await h.serverTicks(40)
  const boat = Object.values(bot.entities).find(e => e.name === 'oak_boat')
  h.result('boat.passengers', boat ? (boat.passengers || []).length : 'no boat')
  h.cmd('kill @e[type=!player]')
  h.result('finished', 'ok')
  process.exit(0)
})

const effects = new Set()
bot._client.on('entity_effect', (p) => {
  if (p.entityId === bot.entity.id) effects.add(p.effectId)
})
