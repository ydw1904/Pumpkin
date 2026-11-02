// A harming cloud set up like a thrown lingering potion's hits a survival player every 20 ticks
// for 3 damage, shrinks by its radius-on-use and vanishes after the fifth hit. A cloud whose
// half-block-high box ends below the player's feet never hits.
const { make } = require('./survival-lib')
const h = make('Cloudy')
const { bot } = h
const X = 1600; const Y = 200; const Z = 0
const CLOUD = '{Radius:3.0f,Duration:600,WaitTime:10,RadiusOnUse:-0.5f,RadiusPerTick:-0.005f,' +
  'potion_contents:{potion:"minecraft:harming"},potion_duration_scale:0.25f}'

h.start(async () => {
  await h.arena(X, Y, Z, 6)
  h.cmd('difficulty normal')
  await h.ticks(20)
  const cx = X + 0.5; const cz = Z + 0.5

  // Starts 1.0 below the feet: the box spans 0.5 blocks up, so the player is out of it.
  let hits = 0
  const healthBefore = bot.health
  const countHit = () => { if (bot.health < healthBefore) hits++ }
  bot.on('health', countHit)
  h.cmd(`summon area_effect_cloud ${cx} ${Y} ${cz} ${CLOUD}`)
  await sleep(3000)
  bot.removeListener('health', countHit)
  h.result('below_feet_hits', hits)
  h.cmd('kill @e[type=minecraft:area_effect_cloud]')
  await h.ticks(10)

  const healths = []
  let last = null
  const intervals = []
  let lastHit = null
  let tick = 0
  const onTick = () => { tick++ }
  const onHealth = () => {
    if (last !== null && bot.health < last) {
      healths.push(bot.health)
      if (lastHit !== null) intervals.push(Math.round((tick - lastHit) / 5) * 5)
      lastHit = tick
    }
    last = bot.health
  }
  last = bot.health
  bot.on('physicsTick', onTick)
  bot.on('health', onHealth)
  h.cmd(`summon area_effect_cloud ${cx} ${Y + 1} ${cz} ${CLOUD}`)
  await h.waitFor(() => !Object.values(bot.entities).some(e => e.name === 'area_effect_cloud') && healths.length > 0, 15000)
  await h.ticks(40)
  bot.removeListener('health', onHealth)
  bot.removeListener('physicsTick', onTick)
  h.result('hit_healths', healths.join(','))
  h.result('hit_intervals', intervals.join(','))
  h.result('cloud_gone', !Object.values(bot.entities).some(e => e.name === 'area_effect_cloud'))
})

function sleep (ms) { return new Promise((resolve) => setTimeout(resolve, ms)) }
