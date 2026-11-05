// Parrots: seeds tame a parrot (one seed per try, smoke until hearts), the owner's click toggles
// sitting, a tamed parrot lands on its owner's shoulder after its ride cooldown and jumps off when
// the owner falls, and a cookie kills a parrot. Flags are read from entity data.
const { make } = require('./survival-lib')
const h = make('Keeper')
const { bot } = h
const X = 9400; const Y = 200; const Z = 0
// Parrot entity data: 18 is the tamable flags (1 sitting, 4 tame). Player: 19 and 20 are the
// shoulder parrots (variant id + 1, 0 for none).
const FLAGS = 18; const SHOULDER_LEFT = 19; const SHOULDER_RIGHT = 20

function parrot () {
  return bot.nearestEntity((e) => e.name === 'parrot')
}

function flags () {
  const p = parrot()
  return p ? (p.metadata[FLAGS] ?? 0) : 'none'
}

function shoulders () {
  return `${bot.entity.metadata[SHOULDER_LEFT] ?? 0}/${bot.entity.metadata[SHOULDER_RIGHT] ?? 0}`
}

function ownerArray () {
  const hex = bot.player.uuid.replace(/-/g, '')
  const ints = []
  for (let i = 0; i < 4; i++) ints.push(Buffer.from(hex.slice(i * 8, i * 8 + 8), 'hex').readInt32BE(0))
  return `[I;${ints.join(',')}]`
}

async function interact () {
  const p = parrot()
  if (!p) return
  bot._client.write('use_entity', { target: p.id, hand: 0, location: { x: 0, y: 0.3, z: 0 }, sneaking: false })
  await h.ticks(2)
}

async function summon (nbt, x = X + 1.5, z = Z + 0.5) {
  h.cmd('kill @e[type=parrot]')
  await h.waitFor(() => !parrot(), 3000)
  await h.ticks(2)
  h.cmd(`summon parrot ${x} ${Y + 1} ${z} ${nbt}`)
  await h.waitFor(parrot, 5000)
  await h.ticks(5)
}

h.start(async () => {
  await h.arena(X, Y, Z, 6)
  h.cmd('weather clear')
  const statuses = []
  bot._client.on('entity_status', (packet) => {
    const p = parrot()
    if (p && packet.entityId === p.id) statuses.push(packet.entityStatus)
  })

  // Taming: one seed per try, event 6 on a miss and 7 on the hit.
  await summon('{NoAI:1b,Variant:2}')
  await h.give('wheat_seeds', 64)
  bot.setQuickBarSlot(0)
  // Mineflayer shows the hit before the tame flag, so stop on the event.
  for (let i = 0; i < 64 && !statuses.includes(7); i++) {
    await interact()
    await h.ticks(2)
  }
  await h.ticks(5)
  h.result('tame.tamed', (flags() & 4) !== 0)
  h.result('tame.one_seed_per_try', 64 - (h.inv().wheat_seeds || 0) === statuses.length)
  h.result('tame.events', statuses.slice(0, -1).every((s) => s === 6) && statuses[statuses.length - 1] === 7)
  // A tame parrot takes no more seeds.
  const seeds = h.inv().wheat_seeds || 0
  await interact()
  h.result('tame.seed_after_tame', (h.inv().wheat_seeds || 0) === seeds)
  await h.clear()

  // Sitting: an owned parrot summoned sitting stands up and sits down on its owner's clicks.
  await summon(`{Owner:${ownerArray()},Sitting:1b}`)
  await h.ticks(20)
  h.result('sit.start', flags())
  await interact()
  await h.ticks(10)
  h.result('sit.stood_up', flags())

  // Shoulder: a glass shaft keeps the parrot inside its owner's box until its 100-tick ride
  // cooldown is over, then it lands on the shoulder.
  h.cmd(`tp Keeper ${X + 0.5} ${Y + 1} ${Z + 0.5}`)
  h.cmd(`fill ${X - 1} ${Y + 1} ${Z - 1} ${X + 1} ${Y + 4} ${Z + 1} glass hollow`)
  h.cmd(`setblock ${X} ${Y + 1} ${Z} air`)
  h.cmd(`setblock ${X} ${Y + 2} ${Z} air`)
  h.cmd(`setblock ${X} ${Y + 3} ${Z} air`)
  await h.ticks(10)
  await summon(`{Owner:${ownerArray()},Variant:3}`, X + 0.5, Z + 0.5)
  h.result('shoulder.before', shoulders())
  await h.waitFor(() => !parrot(), 30000)
  await h.ticks(5)
  h.result('shoulder.landed', shoulders())
  h.result('shoulder.parrot_entity', parrot() ? 'present' : 'gone')
  // Within 20 ticks of landing nothing makes it jump off; a fall afterwards does.
  await h.ticks(30)
  h.cmd(`fill ${X - 1} ${Y + 1} ${Z - 1} ${X + 1} ${Y + 4} ${Z + 1} air`)
  h.cmd(`tp Keeper ${X + 0.5} ${Y + 4} ${Z + 0.5}`)
  await h.waitFor(() => parrot(), 5000)
  await h.ticks(5)
  h.result('shoulder.after_fall', shoulders())
  h.result('shoulder.released', parrot() ? `flags=${flags()} variant=${parrot().metadata[20]}` : 'none')

  // A cookie poisons and kills.
  // Killed parrots drop feathers into the hotbar.
  await summon('{NoAI:1b}')
  h.cmd('kill @e[type=item]')
  await h.clear()
  await h.give('cookie', 1)
  bot.setQuickBarSlot(0)
  await interact()
  await h.ticks(10)
  const p = parrot()
  h.result('cookie.dead', !p || (p.metadata[9] ?? 1) <= 0)
  h.result('cookie.left', h.inv().cookie || 0)
  h.cmd('kill @e[type=parrot]')
})
