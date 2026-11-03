// Damage a survival player takes: armor and toughness, Protection, damage that bypasses armor,
// armor durability per hit, the invulnerability window after a hit, shield blocking (front arc,
// the 5 tick block delay, shield durability, axes disabling it) and the totem of undying.
const { make } = require('./survival-lib')
const h = make('Defender')
const { bot } = h
const X = 1800; const Y = 200; const Z = 0
const r3 = (n) => Math.round(n * 1000) / 1000

const cooldowns = []
// The client reads a bare group as minecraft:<group>, so the namespace isn't compared.
bot._client.on('set_cooldown', (p) => cooldowns.push(`${String(p.cooldownGroup || p.itemID).replace('minecraft:', '')}:${p.cooldownTicks}`))

async function reset () {
  h.cmd(`effect clear ${bot.username}`)
  h.cmd(`clear ${bot.username}`)
  h.cmd(`effect give ${bot.username} instant_health 1 10`)
  h.cmd('kill @e[type=!player]')
  await h.waitFor(() => bot.health === 20 && bot.inventory.items().length === 0, 5000)
  await h.ticks(25)
}

async function wear (pieces) {
  for (const [slot, item] of pieces) h.cmd(`item replace entity ${bot.username} armor.${slot} with ${item}`)
  await h.waitFor(() => [5, 6, 7, 8].filter(i => bot.inventory.slots[i]).length === pieces.length, 5000)
  await h.ticks(5)
}

const armorUsed = () => [5, 6, 7, 8].map(i => bot.inventory.slots[i] ? bot.inventory.slots[i].durabilityUsed : '-').join(',')

// Damage taken from one /damage, read from the health the client is told.
async function hurt (amount, type, by = '') {
  const before = bot.health
  h.cmd(`damage ${bot.username} ${amount} ${type}${by}`)
  await h.waitFor(() => bot.health !== before, 2000)
  await h.ticks(3)
  return r3(before - bot.health)
}

const diamond = [['head', 'diamond_helmet'], ['chest', 'diamond_chestplate'], ['legs', 'diamond_leggings'], ['feet', 'diamond_boots']]
const iron = [['head', 'iron_helmet'], ['chest', 'iron_chestplate'], ['legs', 'iron_leggings'], ['feet', 'iron_boots']]
const enchanted = (set, ench) => set.map(([s, i]) => [s, `${i}[enchantments={${ench}}]`])

h.start(async () => {
  await h.arena(X, Y, Z, 6)
  h.cmd('difficulty normal')
  // Natural regeneration would blur the damage readings.
  h.cmd('gamerule natural_health_regeneration false')
  h.cmd(`tp ${bot.username} ${X + 0.5} ${Y + 1} ${Z + 0.5} 0 0`)

  const armorCases = [
    ['none', []],
    ['diamond', diamond],
    ['iron', iron],
    ['diamond_prot4', enchanted(diamond, 'protection:4')],
    ['iron_chest_only', [iron[1]]],
  ]
  for (const [label, set] of armorCases) {
    await reset()
    await wear(set)
    h.result(`armor.${label}.mob_attack10`, await hurt(10, 'minecraft:mob_attack'))
    h.result(`armor.${label}.durability`, armorUsed())
  }
  await reset()
  await wear(diamond)
  h.result('armor.diamond.magic10', await hurt(10, 'minecraft:magic'))
  h.result('armor.diamond.magic_durability', armorUsed())
  await reset()
  await wear(enchanted(diamond, 'fire_protection:4'))
  h.result('armor.diamond_fireprot4.in_fire6', await hurt(6, 'minecraft:in_fire'))
  await reset()
  await wear(enchanted(diamond, 'blast_protection:4'))
  h.result('armor.diamond_blastprot4.explosion10', await hurt(10, 'minecraft:explosion'))
  await reset()
  h.cmd(`effect give ${bot.username} resistance 30 1`)
  await h.ticks(5)
  h.result('resistance2.generic10', await hurt(10, 'minecraft:generic'))

  // A second hit inside the invulnerability window deals only what exceeds the first.
  await reset()
  const first = await hurt(4, 'minecraft:generic')
  const before = bot.health
  h.cmd(`damage ${bot.username} 3 minecraft:generic`)
  h.cmd(`damage ${bot.username} 7 minecraft:generic`)
  await h.ticks(10)
  h.result('iframes.4_then_3_then_7', `${first} ${r3(before - bot.health)}`)

  // Shield in the off hand against a zombie in front of and behind the player.
  const zombie = async (dz, extra = '') => {
    h.cmd('kill @e[type=zombie]')
    h.cmd(`summon zombie ${X + 0.5} ${Y + 1} ${(Z + 0.5 + dz).toFixed(3)} {NoAI:1b,Silent:1b,Tags:["z"]${extra}}`)
    await h.waitFor(() => Object.values(bot.entities).some(e => e.name === 'zombie'), 5000)
    await h.ticks(2)
  }
  const shieldUsed = () => bot.inventory.slots[45] ? bot.inventory.slots[45].durabilityUsed : 'gone'
  const raise = async (ticks) => {
    bot.activateItem(true)
    await h.ticks(ticks)
  }
  const lower = async () => { bot.deactivateItem(); await h.ticks(5) }
  for (const [label, dz, wait] of [['front', 2, 10], ['behind', -2, 10], ['front_no_delay', 2, 1]]) {
    await reset()
    h.cmd(`item replace entity ${bot.username} weapon.offhand with shield`)
    await h.waitFor(() => bot.inventory.slots[45], 5000)
    await zombie(dz)
    await raise(wait)
    const before = bot.health
    h.cmd(`damage ${bot.username} 5 minecraft:mob_attack by @e[tag=z,limit=1]`)
    await h.ticks(10)
    h.result(`shield.${label}`, `${r3(before - bot.health)} durability=${shieldUsed()}`)
    await lower()
  }
  // An axe-wielding zombie's hit disables the raised shield.
  await reset()
  cooldowns.length = 0
  h.cmd(`item replace entity ${bot.username} weapon.offhand with shield`)
  await h.waitFor(() => bot.inventory.slots[45], 5000)
  await zombie(2, ',equipment:{mainhand:{id:"minecraft:iron_axe",count:1}}')
  await raise(10)
  h.cmd(`damage ${bot.username} 5 minecraft:mob_attack by @e[tag=z,limit=1]`)
  await h.ticks(10)
  h.result('shield.axe_zombie', `${r3(20 - bot.health)} cooldowns=${cooldowns.join(';') || 'none'}`)
  await lower()

  // A custom blocks_attacks component: no block delay, partial blocking, its own durability
  // cost and a longer disable cooldown. The client has to decode it from the server's packets.
  // Shields share a cooldown group, so wait out the axe's 100 tick cooldown first.
  await h.ticks(100)
  const custom = 'shield[blocks_attacks={block_delay_seconds:0,disable_cooldown_scale:2,damage_reductions:[{base:1,factor:0.5}],item_damage:{threshold:0,base:0,factor:2},block_sound:"minecraft:item.shield.block"}]'
  for (const [label, extra] of [['custom', ''], ['custom_axe', ',equipment:{mainhand:{id:"minecraft:iron_axe",count:1}}']]) {
    await reset()
    cooldowns.length = 0
    h.cmd(`item replace entity ${bot.username} weapon.offhand with ${custom}`)
    await h.waitFor(() => bot.inventory.slots[45], 5000)
    await zombie(2, extra)
    await raise(1)
    h.cmd(`damage ${bot.username} 5 minecraft:mob_attack by @e[tag=z,limit=1]`)
    await h.ticks(10)
    h.result(`shield.${label}`, `${r3(20 - bot.health)} durability=${shieldUsed()} cooldowns=${cooldowns.join(';') || 'none'}`)
    await lower()
  }

  // Totem of undying in the off hand.
  await reset()
  h.cmd(`item replace entity ${bot.username} weapon.offhand with totem_of_undying`)
  await h.waitFor(() => bot.inventory.slots[45], 5000)
  h.cmd(`damage ${bot.username} 50 minecraft:generic`)
  await h.ticks(10)
  const effects = Object.entries(bot.entity.effects).map(([id, e]) => `${id}:${e.amplifier}:${Math.round(e.duration / 20) * 20}`).sort().join(',')
  h.result('totem', `health=${r3(bot.health)} offhand=${bot.inventory.slots[45] ? bot.inventory.slots[45].name : 'empty'} effects=${effects}`)
  h.cmd(`effect clear ${bot.username}`)
  await reset()
  h.cmd(`item replace entity ${bot.username} weapon.offhand with totem_of_undying`)
  await h.waitFor(() => bot.inventory.slots[45], 5000)
  h.cmd(`damage ${bot.username} 50 minecraft:out_of_world`)
  await h.ticks(10)
  h.result('totem.void', `dead=${bot.health <= 0}`)
})
