// Leads go through vanilla `Entity.interact`, before a mob's own `mobInteract`: a lead ties any
// mob `canBeLeashed` allows (not enemies, villagers, turtles), an empty hand unties it and drops
// the lead, and a sneaking click moves the player's leashed mobs onto the clicked mob.
const { make } = require('./survival-lib')
const h = make('Leasher')
const { bot } = h
const X = 3100; const Y = 200; const Z = 0

const named = (name) => Object.values(bot.entities).filter(e => e.name === name)
const holders = {}
bot._client.on('attach_entity', (p) => { holders[p.entityId] = p.vehicleId })
const holderOf = (e) => {
  const holder = holders[e.id]
  // Vanilla unlinks with holder 0, Pumpkin with -1; the client treats both as none.
  if (holder === undefined || holder <= 0) return 'none'
  if (holder === bot.entity.id) return 'player'
  const entity = bot.entities[holder]
  return entity ? entity.name : `id${holder}`
}

async function summon (type, dx, nbt = '') {
  h.cmd(`summon ${type} ${X + dx + 0.5} ${Y + 1} ${Z + 2.5} {NoAI:1b${nbt}}`)
  let mob
  await h.waitFor(() => (mob = named(type).find(e => e.position.distanceTo(new h.Vec3(X + dx + 0.5, Y + 1, Z + 2.5)) < 1)), 5000)
  return mob
}

// Removes the mobs and then the loot they dropped, so the bot doesn't pick it up later.
async function reset () {
  h.cmd('kill @e[type=!player]')
  await h.ticks(10)
  h.cmd('kill @e[type=item]')
  await h.ticks(5)
  await h.clear()
}
// Leads in the inventory and on the ground: the bot may already have picked a dropped one up.
const leads = () => (h.inv().lead || 0) + named('item').map(e => e.getDroppedItem()).filter(it => it && it.name === 'lead').length

async function leash (type, nbt) {
  await reset()
  await h.give('lead', 2)
  const mob = await summon(type, 0, nbt)
  if (!mob) { h.result(`${type}.tie`, 'no_mob'); return }
  await bot.lookAt(mob.position.offset(0, mob.height / 2, 0), true)
  await bot.activateEntity(mob)
  await h.ticks(10)
  h.result(`${type}.tie`, `${holderOf(mob)} leads=${h.inv().lead || 0}`)
  if (holderOf(mob) !== 'player') return
  await h.clear()
  await bot.activateEntity(mob)
  await h.ticks(10)
  h.result(`${type}.untie`, `${holderOf(mob)} leads=${leads()}`)
}

async function transfer () {
  await reset()
  await h.give('lead', 1)
  const cow = await summon('cow', -2)
  const wolf = await summon('wolf', 2)
  if (!cow || !wolf) { h.result('transfer', 'no_mob'); return }
  await bot.lookAt(cow.position.offset(0, 0.7, 0), true)
  await bot.activateEntity(cow)
  await h.ticks(10)
  bot.setControlState('sneak', true)
  await h.ticks(5)
  await bot.lookAt(wolf.position.offset(0, 0.4, 0), true)
  await bot.activateEntity(wolf)
  await h.ticks(10)
  bot.setControlState('sneak', false)
  h.result('transfer.cow_holder', holderOf(cow))
}

h.start(async () => {
  await h.arena(X, Y, Z, 6)
  h.cmd(`gamemode survival ${bot.username}`)
  await h.waitFor(() => bot.game.gameMode === 'survival', 5000)
  await h.ticks(20)
  await leash('wolf')
  await leash('iron_golem')
  await leash('hoglin')
  await leash('creeper')
  await leash('villager')
  await leash('turtle')
  await leash('sheep')
  await transfer()
  h.cmd('kill @e[type=!player]')
})
