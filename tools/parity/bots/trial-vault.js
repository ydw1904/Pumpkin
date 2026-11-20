// Trial chambers: a vault wakes up when a player comes within 4 blocks and shows a reward item,
// takes a trial key, ejects its loot one item a second from above the vault, remembers the
// player in its saved data and rejects a second key; a trial spawner detects a player, spawns,
// ejects its reward from above itself and cools down; Bad Omen turns into Trial Omen and makes
// the spawner ominous, which then drops ominous item spawners over the player.
// pumpkin-settings: spawn_protection=0
const { make } = require('./survival-lib')
const h = make('Trialist', 900000)
const { bot, Vec3 } = h
const X = 7800; const Y = 200; const Z = 0
const VX = X; const VY = Y + 1; const VZ = Z + 6

const flat = (j) => {
  if (j === null || j === undefined) return ''
  if (typeof j !== 'object') return String(j)
  if (Object.keys(j).length === 1 && '' in j) return flat(j[''])
  if (j.translate) return j.translate + (j.with ? '(' + j.with.map(flat).join(',') + ')' : '')
  return (j.text ?? '') + (j.extra ? j.extra.map(flat).join('') : '')
}
const plain = (t) => {
  if (t === null || typeof t !== 'object') return t
  if (t.type === 'compound') return Object.fromEntries(Object.entries(t.value).map(([k, v]) => [k, plain(v)]))
  if (t.type === 'list') return (t.value.value || []).map(v => plain({ type: t.value.type, value: v }))
  if (t.type !== undefined && 'value' in t) return plain(t.value)
  return t
}
const msgs = []
bot._client.on('system_chat', (p) => { if (!p.isActionBar) msgs.push(flat(plain(p.content))) })
async function run (c) {
  msgs.length = 0
  bot.chat('/' + c)
  await h.waitFor(() => msgs.length > 0, 3000)
  await h.ticks(5)
  return msgs.join('|') || 'none'
}
const props = (x, y, z) => { const b = h.block(x, y, z); return b ? b.getProperties() : {} }
// Block entity data the vault sends: whether it shows an item and which players it is lit for.
const vaultUpdates = []
bot._client.on('tile_entity_data', (p) => {
  if (!p.location || p.location.x !== VX || p.location.y !== VY || p.location.z !== VZ) return
  const nbt = plain(p.nbtData) || {}
  const shared = nbt.shared_data || {}
  vaultUpdates.push(`${shared.display_item ? 'item' : 'noitem'}/${(shared.connected_players || []).length}`)
})
// Ticks counted by the bot, to time ejections.
let tick = 0
bot.on('physicsTick', () => { tick++ })
const spawned = []
bot.on('entitySpawn', (e) => {
  if (e.name === 'item') spawned.push({ tick, x: e.position.x, y: e.position.y, z: e.position.z })
})
const stateLog = (x, y, z, key) => {
  const log = []
  const timer = setInterval(() => {
    const s = props(x, y, z)[key]
    if (s !== undefined && log[log.length - 1] !== s) log.push(s)
  }, 25)
  return { log, stop: () => clearInterval(timer) }
}
const effect = (name) => {
  const id = Object.keys(bot.registry.effects).find(k => bot.registry.effects[k].name.replace(/([a-z])([A-Z])/g, '$1_$2').toLowerCase() === name)
  return id !== undefined ? (bot.entity.effects || {})[id] : undefined
}

h.start(async () => {
  h.cmd('difficulty normal')
  h.cmd('time set 6000')
  h.cmd('weather clear')
  await h.arena(X, Y, Z, 16)
  h.cmd(`effect give ${bot.username} resistance infinite 4 true`)
  await h.ticks(10)

  // Vault: dormant while the player is 6 blocks away.
  h.cmd(`setblock ${VX} ${VY} ${VZ} vault[facing=north]`)
  await h.waitFor(() => h.blockName(VX, VY, VZ) === 'vault', 5000)
  await h.ticks(40)
  h.result('vault_far_state', props(VX, VY, VZ).vault_state)
  const states = stateLog(VX, VY, VZ, 'vault_state')
  await h.tp(VX + 0.5, VY, VZ - 2.5)
  await h.waitFor(() => props(VX, VY, VZ).vault_state === 'active', 3000)
  await h.ticks(30)
  h.result('vault_near_state', props(VX, VY, VZ).vault_state)
  h.result('vault_shows_item', vaultUpdates.some(u => u.startsWith('item/1')))

  // A wrong item is refused and stays in the hand.
  await h.give('minecraft:stick', 1)
  bot.setQuickBarSlot(0)
  await h.ticks(2)
  await bot.lookAt(new Vec3(VX + 0.5, VY + 0.5, VZ + 0.5), true)
  await bot.activateBlock(h.block(VX, VY, VZ), new Vec3(0, 0, -1), new Vec3(0.5, 0.5, 0))
  await h.ticks(10)
  h.result('vault_stick', h.invStr())
  await h.clear()

  // The key: unlocking, then one item a second from 1.2 above the vault.
  await h.give('minecraft:trial_key', 2)
  await h.ticks(2)
  spawned.length = 0
  await bot.activateBlock(h.block(VX, VY, VZ), new Vec3(0, 0, -1), new Vec3(0.5, 0.5, 0))
  await h.waitFor(() => props(VX, VY, VZ).vault_state === 'active' && spawned.length > 0, 20000)
  await h.ticks(10)
  states.stop()
  h.result('vault_states', states.log.join('>'))
  h.result('vault_keys_left', h.inv().trial_key || 0)
  const ejected = spawned.filter(s => Math.abs(s.x - (VX + 0.5)) < 0.01 && Math.abs(s.z - (VZ + 0.5)) < 0.01)
  h.result('vault_items_from_top_centre', ejected.length > 0 && ejected.length === spawned.length)
  h.result('vault_eject_y', spawned.length ? (spawned[0].y - VY).toFixed(3) : 'none')
  const gaps = spawned.slice(1).map((s, i) => Math.round((s.tick - spawned[i].tick) / 10) * 10)
  h.result('vault_eject_gaps_20', gaps.every(g => g === 20))
  h.result('vault_rewarded_saved', await run(`execute if data block ${VX} ${VY} ${VZ} server_data.rewarded_players[0]`))
  h.result('vault_lit_for_rewarded', vaultUpdates[vaultUpdates.length - 1] || 'none')

  // The same player again: the key stays.
  h.cmd(`kill @e[type=item,x=${X},y=${Y},z=${Z},distance=..30]`)
  await h.ticks(10)
  spawned.length = 0
  await bot.activateBlock(h.block(VX, VY, VZ), new Vec3(0, 0, -1), new Vec3(0.5, 0.5, 0))
  await h.ticks(60)
  h.result('vault_second_key', `${h.inv().trial_key || 0}/${spawned.length}`)

  // Walking away puts it back to sleep.
  await h.tp(X + 0.5, Y + 1, Z - 10.5)
  await h.waitFor(() => props(VX, VY, VZ).vault_state === 'inactive', 3000)
  h.result('vault_left_state', props(VX, VY, VZ).vault_state)
  await h.clear()

  // Trial spawner: one zombie, ejects a consumable, cools down for 100 ticks.
  const SX = X - 8; const SY = Y + 1; const SZ = Z - 8
  h.cmd('gamerule spawn_mobs true')
  h.cmd(`setblock ${SX} ${SY} ${SZ} trial_spawner{target_cooldown_length:100,normal_config:{spawn_potentials:[{data:{entity:{id:"minecraft:zombie"}},weight:1}],total_mobs:1.0f,simultaneous_mobs:1.0f,ticks_between_spawn:20,loot_tables_to_eject:[{data:"minecraft:spawners/trial_chamber/consumables",weight:1}]}}`)
  await h.waitFor(() => h.blockName(SX, SY, SZ) === 'trial_spawner', 5000)
  const spawner = stateLog(SX, SY, SZ, 'trial_spawner_state')
  await h.tp(SX + 4.5, SY, SZ + 0.5)
  const zombie = await h.waitFor(() => Object.values(bot.entities).some(e => e.name === 'zombie'), 15000)
  h.result('spawner_spawned_zombie', zombie)
  spawned.length = 0
  h.cmd(`execute as @e[type=zombie,x=${SX},y=${SY},z=${SZ},distance=..20] run damage @s 1000 minecraft:player_attack by ${bot.username}`)
  // The zombie's own drops land first; the reward comes after the shutter opens.
  await h.waitFor(() => props(SX, SY, SZ).trial_spawner_state === 'waiting_for_reward_ejection', 15000)
  spawned.length = 0
  await h.waitFor(() => props(SX, SY, SZ).trial_spawner_state === 'cooldown', 15000)
  await h.ticks(10)
  const reward = spawned.filter(s => Math.hypot(s.x - (SX + 0.5), s.z - (SZ + 0.5)) < 0.01)
  h.result('spawner_reward_from_top_centre', reward.length > 0 && reward.length === spawned.length)
  h.result('spawner_eject_y', spawned.length ? (spawned[0].y - SY).toFixed(3) : 'none')
  await h.waitFor(() => props(SX, SY, SZ).trial_spawner_state === 'waiting_for_players' || props(SX, SY, SZ).trial_spawner_state === 'active', 30000)
  spawner.stop()
  h.result('spawner_states', spawner.log.filter((s, i) => i > 0 || s !== 'inactive').join('>'))
  h.cmd(`kill @e[type=zombie,x=${X},y=${Y},z=${Z},distance=..40]`)
  h.cmd(`kill @e[type=item,x=${X},y=${Y},z=${Z},distance=..40]`)

  // Bad Omen II near a fresh spawner: Trial Omen for 2 x 18000 ticks and an ominous spawner.
  const OX = X + 8; const OY = Y + 1; const OZ = Z - 8
  await h.tp(X + 0.5, Y + 1, Z + 14.5)
  h.cmd(`setblock ${OX} ${OY} ${OZ} trial_spawner{normal_config:{spawn_potentials:[{data:{entity:{id:"minecraft:zombie"}},weight:1}],total_mobs:1.0f,simultaneous_mobs:1.0f},ominous_config:{spawn_potentials:[{data:{entity:{id:"minecraft:zombie"}},weight:1}],total_mobs:1.0f,simultaneous_mobs:1.0f,ticks_between_spawn:1000}}`)
  h.cmd(`effect give ${bot.username} bad_omen 600 1`)
  await h.waitFor(() => effect('bad_omen') !== undefined, 5000)
  await h.tp(OX + 4.5, OY, OZ + 0.5)
  await h.waitFor(() => props(OX, OY, OZ).ominous === true, 5000)
  await h.ticks(5)
  h.result('ominous_spawner', props(OX, OY, OZ).ominous)
  const trial = effect('trial_omen')
  h.result('trial_omen', trial ? `${trial.amplifier}/${Math.round(trial.duration / 1000)}k` : 'none')
  h.result('bad_omen_gone', effect('bad_omen') === undefined)
  // The ominous spawner drops an item spawner over the player or its mob, which releases its
  // item later.
  const omen = await h.waitFor(() => Object.values(bot.entities).some(e => e.name === 'ominous_item_spawner'), 15000)
  h.result('ominous_item_spawner_seen', omen)
  const itemSpawner = Object.values(bot.entities).find(e => e.name === 'ominous_item_spawner')
  if (itemSpawner) {
    const p = itemSpawner.position
    h.result('ominous_item_spawner_centred', Math.abs(p.x - Math.floor(p.x) - 0.5) < 0.01 && Math.abs(p.z - Math.floor(p.z) - 0.5) < 0.01)
    const start = tick
    await h.waitFor(() => !bot.entities[itemSpawner.id], 10000)
    const life = tick - start
    h.result('ominous_item_spawner_life', life >= 40 && life <= 140)
  }
  h.cmd('gamerule spawn_mobs false')
  h.cmd(`kill @e[type=!player,x=${X},y=${Y},z=${Z},distance=..40]`)
  await h.ticks(10)
  await h.clear()

  // A summoned item spawner inside the bot: the sound 36 ticks before, then the item, which has no
  // pickup delay, and the spawner goes; an arrow is shot straight down instead.
  await h.tp(X + 0.5, Y + 1, Z + 0.5)
  await h.ticks(10)
  const sounds = []
  const soundListener = () => sounds.push(tick)
  bot._client.on('sound_effect', soundListener)
  spawned.length = 0
  const summonedAt = tick
  h.cmd(`summon ominous_item_spawner ${X + 0.5} ${Y + 2} ${Z + 0.5} {item:{id:"minecraft:diamond",count:1},spawn_item_after_ticks:40}`)
  await h.waitFor(() => (h.inv().diamond || 0) > 0, 8000)
  const pickedAt = tick
  await h.ticks(5)
  bot._client.removeListener('sound_effect', soundListener)
  h.result('summoned_sound_before_item', sounds.length > 0 && spawned.length > 0 && sounds[0] < spawned[0].tick)
  h.result('summoned_item_after_40', spawned.length ? Math.round((spawned[0].tick - summonedAt) / 10) * 10 : 'none')
  h.result('summoned_picked_fast', spawned.length > 0 && pickedAt - spawned[0].tick < 5)
  h.result('summoned_spawner_gone', !Object.values(bot.entities).some(e => e.name === 'ominous_item_spawner'))
  const arrows = []
  const arrowListener = (e) => { if (e.name === 'arrow') arrows.push({ vx: e.velocity.x, vy: e.velocity.y }) }
  bot.on('entitySpawn', arrowListener)
  h.cmd(`summon ominous_item_spawner ${X + 4.5} ${Y + 4.5} ${Z + 0.5} {item:{id:"minecraft:arrow",count:1},spawn_item_after_ticks:10}`)
  await h.waitFor(() => arrows.length > 0, 5000)
  await h.ticks(3)
  bot.removeListener('entitySpawn', arrowListener)
  h.result('summoned_arrow_down', arrows.length > 0 && arrows[0].vy < 0 && Math.abs(arrows[0].vx) < 0.1)
  h.cmd(`kill @e[type=!player,x=${X},y=${Y},z=${Z},distance=..40]`)
  await h.ticks(10)
})
