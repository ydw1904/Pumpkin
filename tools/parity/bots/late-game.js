// Mid and late game survival systems: enchanting table bookshelves, item slot and levels;
// smithing sound and recipe error flag; elytra glide wear, aviate and fall stats and rocket
// boosts; beacon effects; conduit power and conduit attacks.
const { make } = require('./survival-lib')
const h = make('Late')
const { bot } = h
const X = 9100; const Y = 200; const Z = 0
// The workstation sits four blocks out so its bookshelf ring stays clear of the bot.
const BX = X + 4; const BY = Y + 1; const BZ = Z
// Vanilla `BuiltInRegistries.STAT_TYPE` order puts custom stats at 8; ids follow custom_stats.json.
const CUSTOM = 8
const CUSTOM_STATS = ['leave_game', 'play_time', 'total_world_time', 'time_since_death', 'time_since_rest',
  'sneak_time', 'walk_one_cm', 'crouch_one_cm', 'sprint_one_cm', 'walk_on_water_one_cm', 'fall_one_cm',
  'climb_one_cm', 'fly_one_cm', 'walk_under_water_one_cm', 'minecart_one_cm', 'boat_one_cm', 'pig_one_cm',
  'happy_ghast_one_cm', 'horse_one_cm', 'aviate_one_cm', 'swim_one_cm']
// `mob_effect` registry ids.
const SPEED = 0; const REGENERATION = 9; const CONDUIT_POWER = 28

const props = {}
bot._client.on('craft_progress_bar', (p) => { props[p.property] = p.value })
const levelEvents = []
bot._client.on('world_event', (p) => levelEvents.push(p.effectId))
const effects = []
bot._client.on('entity_effect', (p) => { if (bot.entity && p.entityId === bot.entity.id) effects.push(p) })
let selfMotion = 0
bot._client.on('entity_velocity', (p) => { if (bot.entity && p.entityId === bot.entity.id) selfMotion++ })

// Every position the bot sends, and the vanilla aviate stat for the moves after a glide request.
const vanillaCm = (d2) => Math.round(Math.fround(Math.fround(Math.sqrt(d2)) * 100))
let lastSent = null
let aviate = null
const write = bot._client.write.bind(bot._client)
bot._client.write = (name, params) => {
  if ((name === 'position' || name === 'position_look') && params) {
    if (aviate && lastSent) {
      const d2 = (params.x - lastSent.x) ** 2 + (params.y - lastSent.y) ** 2 + (params.z - lastSent.z) ** 2
      if (d2 > 0) aviate.round += vanillaCm(d2)
    }
    lastSent = { x: params.x, y: params.y, z: params.z }
  }
  return write(name, params)
}

const stats = {}
async function customStats () {
  const reply = new Promise((resolve) => {
    bot._client.once('statistics', (p) => resolve(p.entries))
    setTimeout(() => resolve([]), 5000)
  })
  bot._client.write('client_command', { actionId: 'request_stats' })
  // Vanilla only sends the stats that changed since the last request.
  for (const e of await reply) if (e.categoryId === CUSTOM && CUSTOM_STATS[e.statisticId]) stats[CUSTOM_STATS[e.statisticId]] = e.value
  return { ...stats }
}
const statDelta = (before, after, name) => (after[name] || 0) - (before[name] || 0)

const place = async (block, x = BX, y = BY, z = BZ) => {
  const name = block.split('[')[0]
  for (let i = 0; i < 5; i++) {
    h.cmd(`setblock ${x} ${y} ${z} air`)
    h.cmd(`setblock ${x} ${y} ${z} ${block}`)
    if (await h.waitFor(() => h.blockName(x, y, z) === name, 2000)) break
  }
  await h.ticks(2)
}
const open = async (block, map, at = [BX, BY, BZ]) => {
  if (bot.currentWindow) { bot.closeWindow(bot.currentWindow); await h.ticks(5) }
  if (block) await place(block, ...at)
  if (map) await h.fill(map)
  for (const k of Object.keys(props)) delete props[k]
  const w = await bot.openBlock(h.block(...at))
  await h.ticks(2)
  await bot._syncWindow(w)
  return w
}
const close = async () => { if (bot.currentWindow) bot.closeWindow(bot.currentWindow); await h.ticks(5) }
const put = async (from, to, button = 0) => { await h.click(from, 0, 0); await h.click(to, button, 0); await h.click(from, 0, 0) }
const inv = (w, i) => w.slots.length - 36 + (i < 9 ? 27 + i : i - 9)
const button = async (id) => {
  bot._client.write('enchant_item', { windowId: bot.currentWindow.id, enchantment: id })
  await h.ticks(3)
  await bot._syncWindow(bot.currentWindow)
}
const setLevels = async (n) => {
  h.cmd(`xp set ${bot.username} 0 points`)
  h.cmd(`xp set ${bot.username} ${n} levels`)
  await h.waitFor(() => bot.experience.level === n, 3000)
}
const ring = (fn) => {
  for (let dy = 0; dy <= 1; dy++) for (let dx = -2; dx <= 2; dx++) for (let dz = -2; dz <= 2; dz++) if (Math.abs(dx) === 2 || Math.abs(dz) === 2) fn(dx, dy, dz)
}
const clearAround = async () => {
  h.cmd(`fill ${BX - 2} ${BY} ${BZ - 2} ${BX + 2} ${BY + 1} ${BZ + 2} air`)
  await h.ticks(5)
}
const scene = async (name, fn) => {
  try { await fn() } catch (e) { h.result(`${name}.exception`, e.message.replace(/\s+/g, ' ').slice(0, 120)) }
}

async function enchanting () {
  // Shelves beside the ring's middle need the block between them and the table along the axis
  // clear (`offset / 2`), not the diagonal one.
  await scene('enchant_side_shelves', async () => {
    await clearAround()
    await place('enchanting_table')
    for (let dy = 0; dy <= 1; dy++) {
      for (const [dx, dz] of [[1, 0], [-1, 0], [0, 1], [0, -1]]) h.cmd(`setblock ${BX + dx} ${BY + dy} ${BZ + dz} stone`)
      for (const [dx, dz] of [[2, 1], [2, -1], [-2, 1], [-2, -1], [1, 2], [-1, 2], [1, -2], [-1, -2]]) h.cmd(`setblock ${BX + dx} ${BY + dy} ${BZ + dz} bookshelf`)
    }
    await h.ticks(5)
    const w = await open(null, { 0: 'iron_sword', 1: 'lapis_lazuli 10' })
    await put(inv(w, 0), 0)
    await h.ticks(5)
    h.result('enchant_side_shelves.offer', `slot3_is_30=${props[2] === 30} costs_le_8=${[0, 1, 2].every(i => props[i] >= 0 && props[i] <= 8)}`)
    await close()
  })
  // A snow layer you can stand on still lets the power through (the tag is all that counts).
  await scene('enchant_snow_transmitter', async () => {
    await clearAround()
    await place('enchanting_table')
    ring((dx, dy, dz) => { if (dy === 0) h.cmd(`setblock ${BX + dx} ${BY + dy} ${BZ + dz} bookshelf`) })
    for (let dx = -1; dx <= 1; dx++) for (let dz = -1; dz <= 1; dz++) if (dx || dz) h.cmd(`setblock ${BX + dx} ${BY} ${BZ + dz} snow[layers=8]`)
    await h.ticks(5)
    const w = await open(null, { 0: 'iron_sword', 1: 'lapis_lazuli 10' })
    await put(inv(w, 0), 0)
    await h.ticks(5)
    h.result('enchant_snow_transmitter.offer', `slot3_is_30=${props[2] === 30}`)
    await close()
  })
  // Bookshelves placed while the menu is open count once the item changes.
  await scene('enchant_recount', async () => {
    await clearAround()
    await place('enchanting_table')
    const w = await open(null, { 0: 'iron_sword', 1: 'lapis_lazuli 10' })
    ring((dx, dy, dz) => { if (dy === 0 && !(dx === -2 && dz === 0)) h.cmd(`setblock ${BX + dx} ${BY + dy} ${BZ + dz} bookshelf`) })
    await h.waitFor(() => h.blockName(BX + 2, BY, BZ + 2) === 'bookshelf', 3000)
    await h.ticks(5)
    await put(inv(w, 0), 0)
    await h.ticks(5)
    h.result('enchant_recount.offer', `slot3_is_30=${props[2] === 30}`)
    await close()
  })
  // The item slot holds one item, by click or shift-click.
  await scene('enchant_item_slot', async () => {
    await clearAround()
    await place('enchanting_table')
    const w = await open(null, { 0: 'book 5', 1: 'lapis_lazuli 10' })
    await h.click(inv(w, 0), 0, 0)
    await h.click(0, 0, 0)
    h.result('enchant_item_slot.click', h.win(0, 2))
    await h.click(inv(w, 0), 0, 0)
    await h.click(0, 0, 1)
    await h.click(inv(w, 0), 0, 1)
    h.result('enchant_item_slot.shift', `${h.win(0, 2)} inv_books=${h.inv().book || 0}`)
    await setLevels(30)
    await put(inv(w, 1), 1)
    await bot._syncWindow(w)
    await button(0)
    h.result('enchant_item_slot.enchanted', `${h.win(0, 2)} level=${bot.experience.level}`)
    await close()
  })
  // Creative players pay the levels but no lapis.
  await scene('enchant_creative', async () => {
    await clearAround()
    await place('enchanting_table')
    h.cmd(`gamemode creative ${bot.username}`)
    await h.waitFor(() => bot.game.gameMode === 'creative', 5000)
    await setLevels(30)
    const w = await open(null, { 0: 'iron_sword', 1: 'lapis_lazuli 10' })
    await put(inv(w, 0), 0)
    await put(inv(w, 1), 1)
    await h.ticks(5)
    const seed = props[3]
    await button(0)
    await h.ticks(5)
    h.result('enchant_creative.after', `level=${bot.experience.level} lapis=${h.stackStr(w.slots[1])} enchanted=${!!(w.slots[0] && w.slots[0].componentMap && w.slots[0].componentMap.has('enchantments'))} seed_changed=${props[3] !== seed}`)
    await close()
    h.cmd(`gamemode survival ${bot.username}`)
    await h.waitFor(() => bot.game.gameMode === 'survival', 5000)
  })
  await clearAround()
}

async function smithing () {
  await scene('smith_error', async () => {
    const s = await open('smithing_table', { 0: 'netherite_upgrade_smithing_template 2', 1: 'iron_chestplate', 2: 'netherite_ingot 2', 3: 'diamond_sword' })
    await put(inv(s, 0), 0)
    await put(inv(s, 1), 1)
    await put(inv(s, 2), 2, 1)
    await h.ticks(3)
    h.result('smith_error.no_recipe', `error=${props[0] ?? '-'} result=${h.stackStr(s.slots[3])}`)
    await h.click(1, 0, 1)
    await put(inv(s, 3), 1)
    await h.ticks(3)
    h.result('smith_error.recipe', `error=${props[0] ?? '-'} result=${h.stackStr(s.slots[3])}`)
    levelEvents.length = 0
    await h.click(3, 0, 1)
    await h.ticks(5)
    h.result('smith_error.taken', `${h.win(0, 4)} use_event=${levelEvents.includes(1044)}`)
    await close()
  })
}

async function elytra () {
  await scene('fall_stats', async () => {
    h.cmd(`effect give ${bot.username} resistance infinite 255 true`)
    await h.tp(X + 0.5, Y + 1, Z + 0.5)
    await h.waitFor(() => bot.entity.onGround, 5000)
    const before = await customStats()
    h.cmd(`tp ${bot.username} ${X + 0.5} ${Y + 12} ${Z + 0.5}`)
    await h.waitFor(() => bot.entity.position.y > Y + 10, 5000)
    await h.waitFor(() => bot.entity.onGround, 10000)
    await h.ticks(10)
    const after = await customStats()
    h.result('fall_stats.deltas', ['fall_one_cm', 'fly_one_cm', 'walk_one_cm', 'aviate_one_cm'].map(n => `${n}=${statDelta(before, after, n)}`).join(' '))
  })

  await scene('glide', async () => {
    await h.fill({ 0: 'firework_rocket 3' })
    h.cmd(`item replace entity ${bot.username} armor.chest with elytra`)
    await h.waitFor(() => bot.inventory.slots[6] && bot.inventory.slots[6].name === 'elytra', 3000)
    bot.setQuickBarSlot(0)
    await customStats()
    await h.tp(X + 0.5, Y + 100, Z + 0.5, 0, 45)
    await h.ticks(5)
    aviate = { round: 0 }
    await bot.elytraFly()
    const flying = await h.waitFor(() => bot.entity.elytraFlying, 3000)
    h.result('glide.started', flying)
    await h.ticks(50)
    const elytra = bot.inventory.slots[6]
    const after = await customStats()
    const counted = aviate
    aviate = null
    h.result('glide.after_50', `still_gliding=${bot.entity.elytraFlying} elytra_damage=${elytra ? elytra.durabilityUsed : 'none'}`)
    const stat = after.aviate_one_cm || 0
    // The first and last moves of the window land a tick apart on the two servers, so only the size is compared.
    h.result('glide.aviate', `near_flown_distance=${Math.abs(stat - counted.round) < 300}`)
    selfMotion = 0
    bot.activateItem()
    await h.ticks(40)
    h.result('glide.rocket', `own_motion_packets=${selfMotion} rockets_left=${h.inv().firework_rocket || 0}`)
    // A ladder ends the glide on the server, not only on the client.
    h.cmd(`fill ${X + 6} ${Y + 1} ${Z + 1} ${X + 6} ${Y + 30} ${Z + 1} stone`)
    h.cmd(`fill ${X + 6} ${Y + 1} ${Z} ${X + 6} ${Y + 30} ${Z} ladder[facing=north]`)
    await h.waitFor(() => h.blockName(X + 6, Y + 20, Z) === 'ladder', 3000)
    const before = bot.entity.elytraFlying
    await h.tp(X + 6.5, Y + 20, Z + 0.5)
    await h.ticks(10)
    h.result('glide.ladder', `before=${before} gliding=${bot.entity.elytraFlying}`)
    await h.tp(X + 0.5, Y + 1, Z + 0.5)
    await h.waitFor(() => bot.entity.onGround, 10000)
    await h.ticks(10)
    h.result('glide.landed', `gliding=${bot.entity.elytraFlying}`)
    h.cmd(`item replace entity ${bot.username} armor.chest with air`)
  })
}

async function beacon () {
  await scene('beacon', async () => {
    const bx = X + 12; const by = Y + 5; const bz = Z
    for (let l = 1; l <= 4; l++) h.cmd(`fill ${bx - l} ${by - l} ${bz - l} ${bx + l} ${by - l} ${bz + l} iron_block`)
    await place('beacon', bx, by, bz)
    await h.serverTicks(200)
    await h.tp(bx + 1.5, by, bz + 0.5)
    const w = await open(null, { 0: 'iron_ingot 2' }, [bx, by, bz])
    h.result('beacon.levels', `levels=${props[0]}`)
    await put(inv(w, 0), 0, 1)
    effects.length = 0
    bot._client.write('set_beacon_effect', { primary_effect: SPEED, secondary_effect: REGENERATION })
    await h.ticks(5)
    h.result('beacon.paid', `payment=${h.stackStr(w.slots[0])} primary=${props[1]} secondary=${props[2]}`)
    await close()
    await h.waitFor(() => effects.length >= 2, 8000)
    h.result('beacon.effects', effects.slice(0, 2).map(e => `${e.effectId}:${e.amplifier}:${e.duration}`).sort().join(' '))
    // Primary and secondary the same: one effect, one level higher.
    const w2 = await open(null, { 0: 'iron_ingot 2' }, [bx, by, bz])
    await put(inv(w2, 0), 0, 1)
    effects.length = 0
    bot._client.write('set_beacon_effect', { primary_effect: SPEED, secondary_effect: SPEED })
    await h.ticks(5)
    await close()
    await h.waitFor(() => effects.length >= 1, 8000)
    await h.serverTicks(100)
    h.result('beacon.doubled', [...new Set(effects.map(e => `${e.effectId}:${e.amplifier}:${e.duration}`))].sort().join(' '))
    // Out of range (50 blocks for four levels) nothing comes.
    await h.tp(bx + 62.5, by, bz + 0.5)
    effects.length = 0
    await h.serverTicks(180)
    h.result('beacon.out_of_range', `effects=${effects.length}`)
    await h.tp(X + 0.5, Y + 1, Z + 0.5)
  })
}

async function conduit () {
  await scene('conduit', async () => {
    const cx = X - 20; const cy = Y + 12; const cz = Z
    h.cmd(`fill ${cx - 4} ${cy - 4} ${cz - 4} ${cx + 4} ${cy + 4} ${cz + 4} glass hollow`)
    h.cmd(`fill ${cx - 3} ${cy - 3} ${cz - 3} ${cx + 3} ${cy + 3} ${cz + 3} water`)
    // The full 42 block frame: every ring block of the three planes through the conduit.
    for (let ox = -2; ox <= 2; ox++) {
      for (let oy = -2; oy <= 2; oy++) {
        for (let oz = -2; oz <= 2; oz++) {
          const ax = Math.abs(ox); const ay = Math.abs(oy); const az = Math.abs(oz)
          if ((ax > 1 || ay > 1 || az > 1) && ((ox === 0 && (ay === 2 || az === 2)) || (oy === 0 && (ax === 2 || az === 2)) || (oz === 0 && (ax === 2 || ay === 2)))) {
            h.cmd(`setblock ${cx + ox} ${cy + oy} ${cz + oz} prismarine`)
          }
        }
      }
    }
    h.cmd(`setblock ${cx} ${cy} ${cz} conduit[waterlogged=true]`)
    h.cmd(`effect give ${bot.username} water_breathing infinite 0 true`)
    effects.length = 0
    await h.tp(cx + 3.5, cy - 3, cz + 3.5)
    h.cmd(`summon drowned ${cx - 2.5} ${cy - 3} ${cz - 2.5} {NoAI:1b,PersistenceRequired:1b,Silent:1b}`)
    const health = []
    bot._client.on('entity_metadata', (p) => {
      const e = bot.entities[p.entityId]
      if (e && e.name === 'drowned') for (const m of p.metadata) if (m.key === 9) health.push(Math.round(m.value * 10) / 10)
    })
    await h.waitFor(() => effects.some(e => e.effectId === CONDUIT_POWER), 8000)
    const power = effects.find(e => e.effectId === CONDUIT_POWER)
    h.result('conduit.power', power ? `${power.amplifier}:${power.duration}` : 'none')
    await h.waitFor(() => health.some(v => v < 20), 8000)
    h.result('conduit.attack', `first_health=${health.find(v => v < 20)}`)
    h.cmd('kill @e[type=drowned]')
    await h.tp(X + 0.5, Y + 1, Z + 0.5)
  })
}

h.start(async () => {
  await h.arena(X, Y, Z, 8)
  h.cmd(`effect give ${bot.username} resistance infinite 255 true`)
  await enchanting()
  await smithing()
  await elytra()
  await beacon()
  await conduit()
})
