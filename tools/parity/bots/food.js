// Eating and drinking: use time, food and saturation gained, the effects foods and potions give,
// the container left behind, milk clearing effects, and natural regeneration on a full bar.
const { make } = require('./survival-lib')
const h = make('Eater')
const { bot } = h
const X = 1900; const Y = 200; const Z = 0
const r2 = (n) => Math.round(n * 100) / 100

let tick = 0
bot.on('physicsTick', () => { tick++ })

const effects = () => Object.values(bot.entity.effects)
  .map(e => `${e.id}:${e.amplifier}:${Math.round(e.duration / 20) * 20}`).sort().join(',') || 'none'

async function starve (to) {
  h.cmd(`effect give ${bot.username} hunger 30 100`)
  await h.waitFor(() => bot.food <= to, 20000)
  h.cmd(`effect clear ${bot.username}`)
  await h.waitFor(() => effects() === 'none', 3000)
  await h.ticks(10)
}

async function consume (item, label = item) {
  h.cmd(`clear ${bot.username}`)
  await h.ticks(2)
  h.cmd(`item replace entity ${bot.username} hotbar.0 with ${item}`)
  bot.setQuickBarSlot(0)
  await h.waitFor(() => bot.heldItem, 5000)
  await h.ticks(5)
  const name = bot.heldItem.name
  const food = bot.food; const sat = bot.foodSaturation
  const start = tick
  bot.activateItem()
  const done = await h.waitFor(() => !bot.heldItem || bot.heldItem.name !== name, 6000)
  const took = tick - start
  bot.deactivateItem()
  await h.ticks(5)
  // Bot ticks drift from server ticks by a few; round to 8. Starving overshoots by a point now
  // and then, so the food level before eating is printed to explain saturation differences.
  h.result(`eat.${label}`, `done=${done} ticks~${Math.round(took / 8) * 8} food=${food}+${bot.food - food} sat=${r2(sat)}->${r2(bot.foodSaturation)} left=${h.invStr()}`)
  // Rotten flesh's hunger is a random roll.
  if (label !== 'rotten_flesh') h.result(`eat.${label}.effects`, effects())
}

h.start(async () => {
  await h.arena(X, Y, Z, 4)
  h.cmd('difficulty normal')
  await h.ticks(20)

  // Natural regeneration with a full bar burns saturation for fast healing.
  const healths = []
  const onHealth = () => healths.push(`${r2(bot.health)}@${tick}`)
  h.result('start', `food=${bot.food} sat=${r2(bot.foodSaturation)}`)
  h.cmd(`damage ${bot.username} 6 minecraft:generic`)
  await h.waitFor(() => bot.health < 20, 3000)
  const t0 = tick
  bot.on('health', onHealth)
  await h.ticks(100)
  bot.removeListener('health', onHealth)
  h.result('regen.full', `health=${r2(bot.health)} food=${bot.food} sat=${r2(bot.foodSaturation)} updates=${healths.length}`)
  h.result('regen.full.first_heals', healths.slice(0, 4).map(s => { const [v, t] = s.split('@'); return `${v}@${Math.round((t - t0) / 5) * 5}` }).join(' '))

  // Slow regeneration at 18 food: one point every 80 ticks.
  h.cmd(`effect give ${bot.username} instant_health 1 5`)
  await starve(18)
  h.cmd(`damage ${bot.username} 4 minecraft:generic`)
  await h.waitFor(() => bot.health < 20, 3000)
  const hp = bot.health
  await h.ticks(200)
  h.result('regen.food18', `healed=${r2(bot.health - hp)} food=${bot.food}`)
  h.cmd(`effect give ${bot.username} instant_health 1 5`)

  const foods = [
    ['apple'], ['bread'], ['cooked_beef'], ['golden_carrot'], ['dried_kelp'], ['cookie'], ['sweet_berries'],
    ['honey_bottle'], ['mushroom_stew'], ['beetroot_soup'], ['rotten_flesh'], ['pufferfish'], ['golden_apple'],
    ['enchanted_golden_apple'],
    ['suspicious_stew[suspicious_stew_effects=[{id:"minecraft:night_vision",duration:100}]]', 'suspicious_stew'],
  ]
  for (const [item, label] of foods) {
    await starve(4)
    await consume(item, label)
    h.cmd(`effect clear ${bot.username}`)
    h.cmd(`effect give ${bot.username} instant_health 1 5`)
    await h.ticks(5)
  }

  await consume('potion[potion_contents={potion:"minecraft:swiftness"}]', 'potion_swiftness')
  await consume('potion[potion_contents={potion:"minecraft:strong_healing"}]', 'potion_strong_healing')
  h.cmd(`effect give ${bot.username} poison 30 0`)
  h.cmd(`effect give ${bot.username} speed 30 0`)
  await h.waitFor(() => Object.keys(bot.entity.effects).length === 2, 3000)
  await consume('milk_bucket')
  // Eating while full is refused unless the food says it can always be eaten.
  await consume('golden_apple', 'golden_apple_full')
  h.cmd(`effect clear ${bot.username}`)
  await consume('bread', 'bread_full')
})
