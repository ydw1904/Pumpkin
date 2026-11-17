// Chunk and entity streaming to a moving player: which chunks the client holds, which it was told
// to forget, cache center updates, the order chunks arrive in, and entities in a far area showing
// up after a long teleport. Movement is the bot's own (client authoritative, like a player flying
// at walk, sprint and elytra speeds) or /tp steps. Only results that don't depend on timing are
// RESULT lines; arrival times are printed as TIMING lines for reading, not compared.
const { make } = require('./survival-lib')
const h = make('Streamer', 900000)
const { bot, Vec3 } = h
const VIEW = 6 // view-distance in both servers' configs
const Y = 230

const key = (x, z) => `${x},${z}`
const chunkOf = (v) => Math.floor(v / 16)
// Vanilla ChunkTrackingView.contains(x, z) (includeNeighbors: a two chunk buffer).
const inView = (cx, cz, vd, x, z) => {
  const dx = Math.max(0, Math.abs(x - cx) - 2)
  const dz = Math.max(0, Math.abs(z - cz) - 2)
  return dx * dx + dz * dz < vd * vd
}
function viewSet (cx, cz, vd) {
  const s = new Set()
  for (let x = cx - vd - 2; x <= cx + vd + 2; x++) for (let z = cz - vd - 2; z <= cz + vd + 2; z++) if (inView(cx, cz, vd, x, z)) s.add(key(x, z))
  return s
}

// Everything the client was told, from login on.
const held = new Map() // chunk key -> time it arrived
let center = null
let lastChunkAt = 0
let log = [] // per scene: {t, kind, ...}
let batch = null
const stats = () => ({ sends: 0, dupSends: 0, dups: [], forgets: 0, forgetUnheld: 0, outsideView: 0, centers: [], batchOrder: 0, batches: 0, events13: 0 })
let s = stats()
bot._client.on('update_view_position', (p) => {
  center = { x: p.chunkX, z: p.chunkZ }
  s.centers.push(key(p.chunkX, p.chunkZ))
  log.push({ t: Date.now(), kind: 'center', x: p.chunkX, z: p.chunkZ })
})
bot._client.on('chunk_batch_start', () => { batch = []; s.batches++ })
bot._client.on('chunk_batch_finished', () => {
  // Vanilla sends each batch nearest first, measured from the player's chunk at send time.
  if (batch && center) {
    const d = batch.map(([x, z]) => (x - center.x) ** 2 + (z - center.z) ** 2)
    for (let i = 1; i < d.length; i++) if (d[i] < d[i - 1]) s.batchOrder++
  }
  batch = null
})
bot._client.on('map_chunk', (p) => {
  const k = key(p.x, p.z)
  s.sends++
  if (held.has(k)) { s.dupSends++; s.dups.push(k) }
  if (center && !inView(center.x, center.z, viewDistance, p.x, p.z)) s.outsideView++
  held.set(k, Date.now())
  lastChunkAt = Date.now()
  if (batch) batch.push([p.x, p.z])
  log.push({ t: lastChunkAt, kind: 'chunk', x: p.x, z: p.z })
})
bot._client.on('unload_chunk', (p) => {
  const k = key(p.chunkX, p.chunkZ)
  s.forgets++
  if (!held.delete(k)) s.forgetUnheld++
  log.push({ t: Date.now(), kind: 'forget', x: p.chunkX, z: p.chunkZ })
})
bot._client.on('position', () => log.push({ t: Date.now(), kind: 'position' }))
bot._client.on('game_state_change', (p) => {
  if (p.reason === 'level_chunks_load_start' || p.reason === 13) { s.events13++; log.push({ t: Date.now(), kind: 'load_start' }) }
})
const entities = new Map() // id -> {x, y, z, t}
bot._client.on('spawn_entity', (p) => {
  entities.set(p.entityId, { x: p.x, y: p.y, z: p.z, t: Date.now(), chunkHeld: held.has(key(chunkOf(p.x), chunkOf(p.z))) })
})
bot._client.on('entity_destroy', (p) => { for (const id of p.entityIds) { const e = entities.get(id); if (e) e.destroyed = Date.now() } })

let viewDistance = VIEW
const here = () => ({ x: chunkOf(bot.entity.position.x), z: chunkOf(bot.entity.position.z) })
function missing () {
  const c = here()
  let n = 0
  for (const k of viewSet(c.x, c.z, viewDistance)) if (!held.has(k)) n++
  return n
}
function outside () {
  const c = here()
  let n = 0
  for (const k of held.keys()) { const [x, z] = k.split(',').map(Number); if (!inView(c.x, c.z, viewDistance, x, z)) n++ }
  return n
}
// Waits until every chunk in view is held and nothing has arrived for a second.
const settle = (ms = 120000) => h.waitFor(() => missing() === 0 && Date.now() - lastChunkAt > 1000, ms)

function begin () { s = stats(); log = [] }
// Reports a scene's counts. `expect` holds the chunk sets the path implies: the union of every
// view along the way minus where it started (sends) and minus where it ended (forgets). When the
// player outruns generation (`noisy`), chunks that pass by before they are ready are never sent,
// so only bounds are compared. Vanilla also sends a forget for a chunk that left the view before
// it was ever sent (PlayerChunkSender.dropChunk only skips pending chunks); the client ignores
// those, so only forgets of held chunks are compared.
function report (name, expect, noisy = false) {
  const r = (k, v) => h.result(`${name}.${k}`, v)
  r('settled', missing() === 0)
  r('missing_in_view', missing())
  r('held_outside_view', outside())
  r('held', held.size)
  // Where a player joins is spread around the world spawn at random, so that one is a count.
  if (name === 'join') r('centers', s.centers.length)
  else r('centers', s.centers.length > 12 ? `${s.centers.length}:${s.centers[0]}..${s.centers[s.centers.length - 1]}` : s.centers.join(' ') || 'none')
  r('sent_outside_view', s.outsideView)
  r('level_chunks_load_start', s.events13)
  const heldForgets = s.forgets - s.forgetUnheld
  if (s.forgetUnheld || s.dupSends) console.log(`NOTE ${name}: ${s.forgetUnheld} forgets for chunks never sent, ${s.dupSends} duplicate sends ${s.dups.join(' ')}`)
  if (noisy) {
    // A /tp lands between vanilla's re-centring and its chunk sending, so a batch can be sorted
    // around a position the client hasn't been told about yet.
    if (s.batchOrder) console.log(`NOTE ${name}: ${s.batchOrder} batch order violations`)
    if (expect) {
      r('sends_at_most_expected', s.sends <= expect.sends)
      r('held_forgets_at_most_expected', heldForgets <= expect.forgets)
    }
    return
  }
  r('batch_order_violations', s.batchOrder)
  r('sends', s.sends)
  r('duplicate_sends', s.dupSends)
  r('held_forgets', heldForgets)
  if (expect) {
    r('sends_expected', s.sends === expect.sends ? 'yes' : `no(${expect.sends})`)
    r('held_forgets_expected', heldForgets === expect.forgets ? 'yes' : `no(${expect.forgets})`)
  }
}
function pathExpect (centers, startView, endView) {
  const union = new Set()
  for (const c of centers) for (const k of viewSet(c.x, c.z, viewDistance)) union.add(k)
  let sends = 0; let forgets = 0
  for (const k of union) { if (!startView.has(k)) sends++; if (!endView.has(k)) forgets++ }
  return { sends, forgets }
}

async function tpSettle (x, z) {
  h.cmd(`tp ${bot.username} ${x} ${Y} ${z}`)
  await h.waitFor(() => Math.abs(bot.entity.position.x - x) < 1 && Math.abs(bot.entity.position.z - z) < 1, 15000)
  return settle()
}

// Flies the bot along +x by `step` blocks per client tick for `n` ticks. The client doesn't move
// while it stands in a chunk it doesn't have, as the vanilla client doesn't.
async function fly (step, n, label) {
  const start = here()
  const startView = viewSet(start.x, start.z, viewDistance)
  const centers = [start]
  let moved = 0; let stalled = 0; let ticks = 0
  const t0 = Date.now()
  await new Promise((resolve) => {
    const onTick = () => {
      bot.entity.velocity.set(0, 0, 0)
      bot.entity.position.x += step
      const c = here()
      const last = centers[centers.length - 1]
      if (c.x !== last.x || c.z !== last.z) centers.push(c)
      if (++moved >= n) { bot.removeListener('physicsTick', onTick); resolve() }
    }
    bot.on('physicsTick', onTick)
    const poll = setInterval(() => {
      ticks++
      if (bot.blockAt(bot.entity.position) == null) stalled++
      if (moved >= n) clearInterval(poll)
    }, 50)
  })
  const flight = Date.now() - t0
  const holesAtStop = missing()
  await settle()
  const end = here()
  console.log(`TIMING ${label}: ${n} ticks took ${flight} ms, ~${stalled} ticks stalled in unloaded chunks, ${holesAtStop} chunks missing when it stopped`)
  return pathExpect(centers, startView, viewSet(end.x, end.z, viewDistance))
}

// Moves by `dx` blocks with /tp every `every` ticks, `n` times.
async function tpSteps (dx, every, n, label) {
  const start = here()
  const startView = viewSet(start.x, start.z, viewDistance)
  const centers = [start]
  let x = bot.entity.position.x
  const z = bot.entity.position.z
  const t0 = Date.now()
  for (let i = 0; i < n; i++) {
    x += dx
    h.cmd(`tp ${bot.username} ${x} ${Y} ${z}`)
    centers.push({ x: chunkOf(x), z: chunkOf(z) })
    await h.sleep(every * 50)
  }
  await h.waitFor(() => Math.abs(bot.entity.position.x - x) < 1, 15000)
  const holesAtStop = missing()
  await settle()
  console.log(`TIMING ${label}: ${n} teleports took ${Date.now() - t0} ms, ${holesAtStop} chunks missing after the last`)
  const end = here()
  return pathExpect(centers, startView, viewSet(end.x, end.z, viewDistance))
}

// The order of what arrived after a teleport until the first chunk: runs of packet kinds.
function sequenceUntilChunk () {
  const out = []
  for (const e of log) {
    if (out[out.length - 1] !== e.kind) out.push(e.kind)
    if (e.kind === 'chunk') break
  }
  return out.join(',')
}

// A long teleport into ungenerated terrain: what is sent around it, and how long the client
// waits for the chunk it stands in.
async function longTp (name, x, z) {
  begin()
  const before = new Set(held.keys())
  const dest = { x: chunkOf(x), z: chunkOf(z) }
  const t0 = Date.now()
  h.cmd(`tp ${bot.username} ${x} ${Y} ${z}`)
  await h.waitFor(() => held.has(key(dest.x, dest.z)), 120000)
  const destAt = Date.now() - t0
  const near = viewSet(dest.x, dest.z, 1)
  await h.waitFor(() => [...near].every(k => held.has(k)), 120000)
  const nearAt = Date.now() - t0
  const firstChunk = log.find(e => e.kind === 'chunk')
  await settle()
  console.log(`TIMING ${name}: destination chunk after ${destAt} ms, its neighbours after ${nearAt} ms, whole view after ${lastChunkAt - t0} ms`)
  h.result(`${name}.sequence`, sequenceUntilChunk())
  // Which chunk finishes generating first is a race on both servers.
  console.log(`TIMING ${name}: first chunk is the destination: ${!!firstChunk && firstChunk.x === dest.x && firstChunk.z === dest.z}`)
  h.result(`${name}.old_view_forgotten`, [...before].filter(k => held.has(k)).length === 0)
  const firstNew = log.findIndex(e => e.kind === 'chunk')
  h.result(`${name}.forgets_before_first_chunk`, log.slice(firstNew).filter(e => e.kind === 'forget').length === 0)
  report(name, { sends: viewSet(dest.x, dest.z, viewDistance).size, forgets: before.size })
}

// Holds back the client's teleport confirmation (and the moves after it) while `fn` runs, like a
// client that is slow to answer. Vanilla's ChunkMap.tick re-centres every player each tick, so the
// view follows a teleport before the client confirms it.
async function withoutConfirm (fn) {
  const write = bot._client.write.bind(bot._client)
  const confirms = []
  bot._client.write = (name, params) => {
    // Moves are dropped: several at once would break the one-move-per-tick rule.
    if (name === 'teleport_confirm') confirms.push(params)
    else if (!['position', 'position_look', 'look', 'flying'].includes(name)) write(name, params)
  }
  try { await fn() } finally {
    bot._client.write = write
    for (const params of confirms) write('teleport_confirm', params)
  }
}

h.start(async () => {
  h.result('join.level_chunks_load_start', s.events13)
  h.result('join.first_center', log.find(e => e.kind === 'center') ? 'before_first_chunk:' + (log.findIndex(e => e.kind === 'center') < log.findIndex(e => e.kind === 'chunk')) : 'none')
  await settle()
  report('join')
  h.cmd(`gamemode creative ${bot.username}`)
  await h.waitFor(() => bot.game.gameMode === 'creative', 5000)
  bot.creative.startFlying()

  // Each speed gets its own line through fresh terrain, starting from a settled view.
  const scenes = [
    ['walk', 0.216, 300, 1000, false],
    ['sprint', 0.28, 300, 2000, false],
    ['sprint_fly', 1.09, 200, 3000, false],
    ['elytra', 1.6, 200, 4000, true],
    ['elytra_fast', 3.0, 120, 5000, true]
  ]
  for (const [name, step, n, z, noisy] of scenes) {
    await tpSettle(0.5, z + 0.5)
    await h.ticks(5)
    begin()
    report(name, await fly(step, n, name), noisy)
  }

  await tpSettle(0.5, 6000.5)
  begin()
  report('tp_steps_8', await tpSteps(8, 2, 40, 'tp_steps_8'), true)
  await tpSettle(0.5, 6500.5)
  begin()
  report('tp_steps_48', await tpSteps(48, 10, 8, 'tp_steps_48'), true)

  // The client asks for a smaller, then a larger view distance than the server's.
  begin()
  bot.setSettings({ viewDistance: 3 })
  viewDistance = 3
  await h.waitFor(() => outside() === 0, 10000)
  await h.sleep(1000)
  const c = here()
  report('view_3', { sends: 0, forgets: viewSet(c.x, c.z, VIEW).size - viewSet(c.x, c.z, 3).size })
  begin()
  bot.setSettings({ viewDistance: 12 })
  viewDistance = VIEW
  await settle()
  report('view_12', { sends: viewSet(c.x, c.z, VIEW).size - viewSet(c.x, c.z, 3).size, forgets: 0 })

  // Long teleports, and pre-placed mobs in the far area showing up when the player comes back.
  await longTp('long_tp', 10000.5, 0.5)
  const pigs = [0, 1, 2].map(i => ({ x: 10024.5, y: Y - 10 + i, z: 0.5 + 3 * i }))
  for (const p of pigs) h.cmd(`summon pig ${p.x} ${p.y} ${p.z} {NoAI:1b,NoGravity:1b,Silent:1b,PersistenceRequired:1b}`)
  const pigIds = () => [...entities.entries()].filter(([, e]) => pigs.some(p => Math.abs(e.x - p.x) < 0.01 && Math.abs(e.y - p.y) < 0.01 && Math.abs(e.z - p.z) < 0.01))
  await h.waitFor(() => pigIds().length >= 3, 10000)
  h.result('pigs.seen_after_summon', pigIds().length)
  const firstIds = pigIds().map(([id]) => id)

  await longTp('long_tp_away', 20000.5, 0.5)
  h.result('pigs.destroyed_on_leave', firstIds.filter(id => entities.get(id).destroyed).length)
  // Give the server time to unload and save the far area.
  await h.sleep(20000)

  const t0 = Date.now()
  await longTp('long_tp_back', 10000.5, 0.5)
  const back = pigIds().filter(([id]) => !firstIds.includes(id))
  await h.waitFor(() => pigIds().filter(([id]) => !firstIds.includes(id)).length >= 3, 30000)
  const again = pigIds().filter(([id]) => !firstIds.includes(id))
  h.result('pigs.seen_after_return', again.length)
  h.result('pigs.spawned_before_their_chunk', again.filter(([, e]) => !e.chunkHeld).length)
  if (again.length) console.log(`TIMING pigs: visible ${Math.max(...again.map(([, e]) => e.t)) - t0} ms after the teleport back (${back.length} by the time the view settled)`)

  // Back to the spawn area (generated at join) without confirming the teleport for 5 s. Vanilla
  // re-centres the view in ChunkMap.tick (center and forgets go out at once) but moves the
  // player's tickets only on a client move, so the destination waits for the confirmation.
  begin()
  const spawnChunk = { x: 0, z: 0 }
  await withoutConfirm(async () => {
    h.cmd(`tp ${bot.username} 0.5 ${Y} 0.5`)
    await h.sleep(5000)
    h.result('unconfirmed_tp.center_before_confirm', s.centers.includes(key(spawnChunk.x, spawnChunk.z)))
    h.result('unconfirmed_tp.forgets_before_confirm', s.forgets > 0)
    h.result('unconfirmed_tp.destination_before_confirm', held.has(key(spawnChunk.x, spawnChunk.z)))
  })
  await settle()
  report('unconfirmed_tp')
})
