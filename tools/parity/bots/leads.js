// Leads go through vanilla `Entity.interact`, before a mob's own `mobInteract`: a lead ties any
// mob `canBeLeashed` allows (not enemies, villagers, turtles), an empty hand unties it and drops
// the lead, and a sneaking click moves the player's leashed mobs onto the clicked mob. Boats are
// leashable too: tied and untied before boarding, pulled by the lead, tied to a fence, and
// snapped when the holder gets too far away.
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

async function boatPull () {
  await reset()
  await h.give('lead', 1)
  // A strip of floor for the pull and the snap.
  h.cmd(`fill ${X + 6} ${Y} ${Z} ${X + 30} ${Y} ${Z + 4} stone`)
  const boat = await summon('oak_boat', 0)
  if (!boat) { h.result('boat_pull', 'no_boat'); return }
  await bot.lookAt(boat.position.offset(0, 0.3, 0), true)
  await bot.activateEntity(boat)
  await h.ticks(10)
  h.result('boat_pull.tie', holderOf(boat))
  await h.tp(X + 10.5, Y + 1, Z + 2.5)
  await h.ticks(60)
  h.result('boat_pull.pulled', `${holderOf(boat)} moved=${boat.position.x - X - 0.5 > 1}`)
  // A fence takes the player's leashed boat.
  h.cmd(`setblock ${X + 12} ${Y + 1} ${Z + 4} oak_fence`)
  await h.ticks(5)
  await bot.lookAt(new h.Vec3(X + 12.5, Y + 1.5, Z + 4.5), true)
  await bot.activateBlock(h.block(X + 12, Y + 1, Z + 4)).catch(e => h.result('boat_pull.fence_error', e.message))
  await h.ticks(10)
  h.result('boat_pull.fence', holderOf(boat))
  // Back on the player, then too far away: the lead snaps and drops.
  const knot = named('leash_knot')[0]
  if (knot) {
    await bot.activateEntity(knot)
    await h.ticks(10)
  }
  h.result('boat_pull.from_knot', holderOf(boat))
  await h.tp(X + 28.5, Y + 1, Z + 2.5)
  await h.ticks(20)
  h.result('boat_pull.snap', `${holderOf(boat)} leads=${leads()}`)
  h.cmd(`fill ${X + 6} ${Y} ${Z} ${X + 30} ${Y + 1} ${Z + 4} air`)
  await h.tp(X + 0.5, Y + 1, Z + 0.5)
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
  await leash('oak_boat')
  await leash('bamboo_chest_raft')
  await boatPull()
  h.cmd('kill @e[type=!player]')
})
