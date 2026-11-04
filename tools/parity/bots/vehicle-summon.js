// Vehicles summoned on top of the player: the server keeps ticking while a boat, chest boat or
// minecart overlaps its summoner, and a sneaking right click opens the chest vehicles. Then
// mineflayer's activateEntity, which sends an interact that isn't sneaking even with sneak held,
// mounts a chest boat; mineflayer stops its own physics ticks while riding, so ticking is checked
// against the server clock.
const { make } = require('./survival-lib')
const h = make('Summoner')
const { bot } = h
const X = 3500; const Y = 200; const Z = 0

const nearest = (name) => bot.nearestEntity((e) => e.name === name)
const ticking = async () => {
  const start = bot.time.age
  await h.waitFor(() => bot.time.age - start >= 40, 10000)
  return bot.time.age - start >= 40
}

h.start(async () => {
  await h.arena(X, Y, Z, 6)
  for (const name of ['oak_chest_boat', 'oak_boat', 'chest_minecart', 'hopper_minecart', 'minecart']) {
    await h.tp(X + 0.5, Y + 1, Z + 0.5)
    h.cmd(`execute at ${bot.username} run summon ${name} ~ ~ ~`)
    h.result(`${name}.seen`, await h.waitFor(() => nearest(name), 5000))
    h.result(`${name}.ticking`, await ticking())
    const vehicle = nearest(name)
    if (vehicle) {
      bot.setControlState('sneak', true)
      await h.ticks(2)
      bot._client.write('use_entity', { target: vehicle.id, hand: 0, location: { x: 0, y: 0.3, z: 0 }, sneaking: true })
      const opened = await h.waitFor(() => bot.currentWindow, 3000)
      h.result(`${name}.sneak_open`, opened ? bot.currentWindow.type : 'none')
      if (bot.currentWindow) bot.closeWindow(bot.currentWindow)
      bot.setControlState('sneak', false)
      h.result(`${name}.ticking_after_open`, await ticking())
    }
    h.cmd(`kill @e[type=${name}]`)
    await h.waitFor(() => !nearest(name), 5000)
  }

  // mineflayer's activateEntity: sneak held, but the interact packet says not sneaking.
  await h.tp(X + 0.5, Y + 1, Z + 0.5)
  h.cmd(`summon oak_chest_boat ${X + 0.5} ${Y + 1} ${Z - 1.5} {LootTable:"minecraft:chests/simple_dungeon"}`)
  await h.waitFor(() => nearest('oak_chest_boat'), 5000)
  await h.ticks(5)
  bot.setControlState('sneak', true)
  await h.ticks(3)
  const events = []
  bot.on('mount', () => events.push('mount'))
  bot._client.on('position', () => events.push('position'))
  await bot.activateEntity(nearest('oak_chest_boat')).catch(() => {})
  await h.sleep(3000)
  h.result('activate.window', bot.currentWindow ? bot.currentWindow.type : 'none')
  // Vanilla teleports the rider onto the seat before it sends the passengers.
  h.result('activate.teleported_then_mounted', events.indexOf('position') >= 0 && events.indexOf('position') < events.indexOf('mount'))
  h.result('activate.riding', !!bot.vehicle)
  if (bot.currentWindow) bot.closeWindow(bot.currentWindow)
  const before = Date.now()
  await Promise.race([h.ticks(2), h.sleep(5000)])
  h.result('activate.bot_ticks', Date.now() - before < 4900)
  bot.setControlState('sneak', false)
  h.result('activate.ticking', await ticking())
})
