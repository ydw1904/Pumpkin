// The entity data a client gets with a freshly summoned entity. Vanilla sends only the values
// that differ from their defaults (`getNonDefaultValues`), so any index Pumpkin adds here is a
// default it sends anyway. Prints the indices seen in the first ticks, with their values.
const { make } = require('./survival-lib')
const h = make('Census')
const { bot } = h
const X = 9800; const Y = 200; const Z = 0

const TYPES = [
  'allay', 'armadillo', 'axolotl', 'bat', 'bee', 'blaze', 'bogged', 'breeze', 'camel', 'cat',
  'cave_spider', 'chicken', 'cod', 'copper_golem', 'cow', 'creaking', 'creeper', 'dolphin',
  'donkey', 'drowned', 'elder_guardian', 'enderman', 'endermite', 'evoker', 'fox', 'frog',
  'ghast', 'glow_squid', 'goat', 'guardian', 'happy_ghast', 'hoglin', 'horse', 'husk',
  'illusioner', 'iron_golem', 'llama', 'magma_cube', 'mooshroom', 'mule', 'nautilus', 'ocelot',
  'panda', 'parrot', 'phantom', 'pig', 'piglin', 'piglin_brute', 'pillager', 'polar_bear',
  'pufferfish', 'rabbit', 'ravager', 'salmon', 'sheep', 'shulker', 'silverfish', 'skeleton',
  'skeleton_horse', 'slime', 'sniffer', 'snow_golem', 'spider', 'squid', 'stray', 'strider',
  'tadpole', 'trader_llama', 'tropical_fish', 'turtle', 'vex', 'villager', 'vindicator',
  'wandering_trader', 'warden', 'witch', 'wither_skeleton', 'wolf', 'zoglin', 'zombie',
  'zombie_horse', 'zombie_villager', 'zombified_piglin', 'armor_stand', 'item_frame',
  'glow_item_frame', 'end_crystal', 'experience_orb', 'falling_block', 'tnt', 'boat', 'minecart',
  'area_effect_cloud', 'interaction', 'block_display', 'item_display', 'text_display', 'arrow',
  'snowball', 'egg', 'fireball', 'small_fireball', 'wither_skull', 'dragon_fireball'
]

const show = (v) => {
  const s = typeof v === 'object' ? JSON.stringify(v) : String(v)
  return s.length > 40 ? s.slice(0, 40) + '...' : s
}

h.start(async () => {
  await h.arena(X, Y, Z, 6)
  h.cmd(`gamemode spectator ${bot.username}`)
  h.cmd('time set noon')
  h.cmd('gamerule advance_time false')
  h.cmd('difficulty peaceful')
  await h.ticks(10)
  h.cmd('difficulty normal')
  const seen = new Map()
  bot._client.on('entity_metadata', (p) => {
    const entry = seen.get(p.entityId)
    if (!entry) return
    for (const m of p.metadata) if (!entry.has(m.key)) entry.set(m.key, show(m.value))
  })
  bot._client.on('spawn_entity', (p) => { if (!seen.has(p.entityId)) seen.set(p.entityId, new Map()) })
  for (const type of TYPES) {
    h.cmd('kill @e[type=!player]')
    await h.ticks(5)
    seen.clear()
    // NoAI and fixed rotation, so nothing a mob does in its first ticks shows up.
    h.cmd(`summon ${type} ${X + 0.5} ${Y + 1} ${Z + 3.5} {NoAI:1b,Silent:1b,PersistenceRequired:1b,Rotation:[0f,0f]}`)
    await h.ticks(10)
    const found = [...seen.values()].find(m => m.size > 0) || new Map()
    const keys = [...found.keys()].sort((a, b) => a - b)
    h.result(`${type}.indices`, keys.join(',') || 'none')
    h.result(`${type}.values`, keys.map(k => `${k}=${found.get(k)}`).join(' ') || 'none')
  }
  h.cmd('kill @e[type=!player]')
})
