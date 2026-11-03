// Farming and animals: which blocks take bone meal and what it does, growth limits under fast
// random ticks (crops, farmland moisture and drying, sugar cane, cactus, bamboo, kelp),
// trampling farmland, breeding with food and the love cooldown, feeding babies, milking cows,
// mooshroom stew and shearing, and shearing sheep.
const { make } = require('./survival-lib')
const h = make('Farmer')
const { bot, Vec3 } = h
const X = 5600; const Y = 200; const Z = 0

const stateStr = (b) => b ? b.name + JSON.stringify(b.getProperties()) : 'unloaded'
async function hold (name, n = 1) {
  if ((h.inv()[name] || 0) < n) await h.give(name, n)
  await bot.equip(bot.inventory.items().find(i => i.name === name), 'hand')
}
async function useOn (x, y, z) {
  await bot.lookAt(new Vec3(x + 0.5, y + 0.5, z + 0.5), true)
  await bot.activateBlock(h.block(x, y, z), new Vec3(0, 1, 0)).catch(e => h.result('use_error', e.message))
  await h.ticks(6)
}
const height = (x, y, z, name) => { let n = 0; while (h.blockName(x, y + n, z) === name) n++; return n }
const mobs = (name, r = 10) => Object.values(bot.entities).filter(e => e.name === name && e.position.distanceTo(bot.entity.position) < r)
const isBaby = (e) => e.metadata && e.metadata[16] === true

// [name, support, block to place, bone meal always changes it]
const MEAL = [
  ['wheat', 'farmland', 'wheat', true],
  ['carrots', 'farmland', 'carrots', true],
  ['potatoes', 'farmland', 'potatoes', true],
  ['beetroots', 'farmland', 'beetroots', false],
  ['melon_stem', 'farmland', 'melon_stem', true],
  ['pumpkin_stem', 'farmland', 'pumpkin_stem', true],
  ['wheat_mature', 'farmland', 'wheat[age=7]', true],
  ['torchflower_crop', 'farmland', 'torchflower_crop', true],
  ['pitcher_crop', 'farmland', 'pitcher_crop', true],
  ['sweet_berry_bush', 'grass_block', 'sweet_berry_bush', true],
  ['nether_wart', 'soul_sand', 'nether_wart', true],
  ['oak_sapling', 'dirt', 'oak_sapling', false],
  ['short_grass', 'grass_block', 'short_grass', true],
  ['dandelion', 'grass_block', 'dandelion', true],
  ['sugar_cane', 'sand', 'sugar_cane', true],
  ['cactus', 'sand', 'cactus', true],
  ['bamboo_sapling', 'dirt', 'bamboo_sapling', true],
  ['red_mushroom', 'mycelium', 'red_mushroom', false],
  ['crimson_fungus', 'crimson_nylium', 'crimson_fungus', false],
  ['pink_petals', 'grass_block', 'pink_petals', true],
  ['sunflower', 'grass_block', 'sunflower', true]
]

h.start(async () => {
  await h.arena(X, Y, Z, 18)
  await h.clear()
  h.cmd('time set 6000')
  h.cmd('weather clear')
  // Bone meal on one block of each kind; water next to the sugar cane.
  MEAL.forEach(([, support, block], i) => {
    const x = X - 17 + i * 2 - (i > 16 ? 34 : 0); const z = i > 16 ? Z - 14 : Z + 3
    h.cmd(`setblock ${x} ${Y} ${z} ${support}`)
    h.cmd(`setblock ${x} ${Y + 1} ${z} ${block}`)
    if (block === 'sugar_cane') h.cmd(`setblock ${x} ${Y} ${z + 1} water`)
    if (block === 'sunflower') h.cmd(`setblock ${x} ${Y + 2} ${z} sunflower[half=upper]`)
  })
  h.cmd(`setblock ${X + 17} ${Y + 1} ${Z + 4} jungle_log`)
  h.cmd(`setblock ${X + 17} ${Y + 1} ${Z + 3} cocoa[facing=south]`)
  await h.ticks(20)
  const all = MEAL.map((m, i) => [m, X - 17 + i * 2 - (i > 16 ? 34 : 0), i > 16 ? Z - 14 : Z + 3]).concat([[['cocoa', '', '', true], X + 17, Z + 3]])
  for (const [[name, , , sure], x, z] of all) {
    await h.tp(x + 0.5, Y + 1, z + (z > Z ? -1.5 : 2.5))
    await hold('bone_meal', 2)
    const before = h.inv().bone_meal
    const s0 = stateStr(h.block(x, Y + 1, z))
    await useOn(x, Y + 1, z)
    const used = before - (h.inv().bone_meal || 0)
    const changed = stateStr(h.block(x, Y + 1, z)) !== s0
    h.result(`bonemeal_${name}`, sure ? `used=${used} changed=${changed}` : `used=${used}`)
    if (name === 'wheat') h.result('bonemeal_wheat_age_2_to_5', [2, 3, 4, 5].includes(Number(h.block(x, Y + 1, z).getProperties().age)))
  }
  h.cmd(`fill ${X - 18} ${Y + 1} ${Z - 15} ${X + 18} ${Y + 8} ${Z + 5} air`)
  h.cmd(`fill ${X - 18} ${Y} ${Z - 15} ${X + 18} ${Y} ${Z + 5} stone`)
  await h.ticks(10)

  // Growth under fast random ticks.
  const G = Z - 6
  h.cmd(`setblock ${X} ${Y} ${G} water`)
  h.cmd(`fill ${X + 1} ${Y} ${G} ${X + 4} ${Y} ${G} farmland`)
  h.cmd(`fill ${X + 1} ${Y + 1} ${G} ${X + 4} ${Y + 1} ${G} wheat`)
  h.cmd(`setblock ${X + 10} ${Y} ${G} farmland`)
  h.cmd(`setblock ${X + 12} ${Y} ${G} farmland`)
  h.cmd(`setblock ${X + 12} ${Y + 1} ${G} wheat`)
  h.cmd(`setblock ${X - 6} ${Y} ${G} sand`)
  h.cmd(`setblock ${X - 6} ${Y} ${G - 1} water`)
  h.cmd(`setblock ${X - 6} ${Y + 1} ${G} sugar_cane`)
  h.cmd(`setblock ${X - 9} ${Y} ${G} sand`)
  h.cmd(`setblock ${X - 9} ${Y + 1} ${G} cactus`)
  h.cmd(`setblock ${X - 12} ${Y} ${G} dirt`)
  h.cmd(`setblock ${X - 12} ${Y + 1} ${G} bamboo_sapling`)
  h.cmd(`fill ${X + 14} ${Y - 7} ${G - 1} ${X + 16} ${Y} ${G + 1} stone`)
  h.cmd(`fill ${X + 15} ${Y - 6} ${G} ${X + 15} ${Y} ${G} water`)
  h.cmd(`setblock ${X + 15} ${Y - 6} ${G} kelp`)
  await h.tp(X + 0.5, Y + 1, Z - 2.5)
  await h.ticks(20)
  h.cmd('gamerule random_tick_speed 1000')
  await h.ticks(600)
  h.cmd('gamerule random_tick_speed 3')
  await h.ticks(10)
  const ages = []; const moist = []
  for (let x = X + 1; x <= X + 4; x++) { ages.push(h.block(x, Y + 1, G).getProperties().age); moist.push(h.block(x, Y, G).getProperties().moisture) }
  h.result('grow_wheat_ages', ages.join(','))
  h.result('grow_farmland_moisture', moist.join(','))
  h.result('grow_dry_farmland_bare', h.blockName(X + 10, Y, G))
  h.result('grow_dry_farmland_crop', h.blockName(X + 12, Y, G))
  h.result('grow_sugar_cane_height', height(X - 6, Y + 1, G, 'sugar_cane'))
  h.result('grow_cactus', `${height(X - 9, Y + 1, G, 'cactus')} top=${h.blockName(X - 9, Y + 1 + height(X - 9, Y + 1, G, 'cactus'), G)}`)
  const bamboo = height(X - 12, Y + 1, G, 'bamboo')
  h.result('grow_bamboo_12_to_16', bamboo >= 12 && bamboo <= 16)
  let kelp = 0; while (['kelp', 'kelp_plant'].includes(h.blockName(X + 15, Y - 6 + kelp, G))) kelp++
  h.result('grow_kelp_height', kelp)

  // Trampling: a two block fall onto farmland turns it to dirt and pops the crop.
  h.cmd(`setblock ${X + 6} ${Y} ${Z + 6} farmland`)
  h.cmd(`setblock ${X + 6} ${Y + 1} ${Z + 6} wheat[age=3]`)
  await h.ticks(5)
  h.cmd(`tp ${bot.username} ${X + 6.5} ${Y + 3} ${Z + 6.5}`)
  await h.ticks(40)
  h.result('trample', `${h.blockName(X + 6, Y, Z + 6)} ${h.blockName(X + 6, Y + 1, Z + 6)}`)

  // Breeding inside a glass pen.
  const P = Z + 12
  h.cmd(`fill ${X - 4} ${Y + 1} ${P - 3} ${X + 4} ${Y + 2} ${P + 3} glass`)
  h.cmd(`fill ${X - 3} ${Y + 1} ${P - 2} ${X + 3} ${Y + 2} ${P + 2} air`)
  await h.tp(X + 0.5, Y + 1, P + 0.5)
  h.cmd(`summon cow ${X - 1.5} ${Y + 1} ${P + 0.5}`)
  h.cmd(`summon cow ${X + 2.5} ${Y + 1} ${P + 0.5}`)
  await h.waitFor(() => mobs('cow').length === 2, 5000)
  await hold('wheat', 10)
  for (const cow of mobs('cow')) { await bot.lookAt(cow.position.offset(0, 1, 0), true); await bot.activateEntity(cow).catch(() => {}); await h.ticks(5) }
  h.result('breed_wheat_used', 10 - (h.inv().wheat || 0))
  await h.ticks(300)
  const cows = mobs('cow')
  h.result('breed_cows', `${cows.length} babies=${cows.filter(isBaby).length}`)
  h.result('breed_xp_orb', h.orbs().length > 0 || bot.experience.points > 0)
  await hold('wheat', 10)
  const before = h.inv().wheat
  const parent = cows.find(c => !isBaby(c))
  if (parent) { await bot.lookAt(parent.position.offset(0, 1, 0), true); await bot.activateEntity(parent).catch(() => {}); await h.ticks(5) }
  h.result('breed_parent_on_cooldown_used', before - (h.inv().wheat || 0))
  const baby = cows.find(isBaby)
  const before2 = h.inv().wheat
  if (baby) { await bot.lookAt(baby.position.offset(0, 0.5, 0), true); await bot.activateEntity(baby).catch(() => {}); await h.ticks(5) }
  h.result('feed_baby_used', before2 - (h.inv().wheat || 0))

  // Milk, mooshroom stew and shearing.
  h.cmd('kill @e[type=cow]')
  h.cmd('kill @e[type=item]')
  await h.ticks(5)
  await h.clear()
  h.cmd(`summon cow ${X + 1.5} ${Y + 1} ${P + 0.5} {NoAI:1b}`)
  h.cmd(`summon mooshroom ${X - 1.5} ${Y + 1} ${P + 0.5} {NoAI:1b}`)
  h.cmd(`summon sheep ${X + 0.5} ${Y + 1} ${P + 2.5} {NoAI:1b,Color:14b}`)
  await h.ticks(20)
  await hold('bucket')
  const cow = mobs('cow')[0]
  if (cow) { await bot.lookAt(cow.position.offset(0, 1, 0), true); await bot.activateEntity(cow).catch(() => {}); await h.ticks(5) }
  h.result('milk', h.invStr())
  await h.clear()
  await hold('bowl')
  const moo = mobs('mooshroom')[0]
  if (moo) { await bot.lookAt(moo.position.offset(0, 1, 0), true); await bot.activateEntity(moo).catch(() => {}); await h.ticks(5) }
  h.result('mooshroom_bowl', h.invStr())
  await h.clear()
  await hold('shears')
  if (moo) { await bot.lookAt(moo.position.offset(0, 1, 0), true); await bot.activateEntity(moo).catch(() => {}); await h.ticks(10) }
  const drops = Object.values(bot.entities).filter(e => e.name === 'item' && e.getDroppedItem()).map(e => `${e.getDroppedItem().name}x${e.getDroppedItem().count}`)
  h.result('mooshroom_shear', `mooshrooms=${mobs('mooshroom').length} cows=${mobs('cow').length} drops=${drops.sort().join(',')}`)
  h.cmd('kill @e[type=item]')
  await h.ticks(5)
  const sheep = mobs('sheep')[0]
  if (sheep) { await bot.lookAt(sheep.position.offset(0, 1, 0), true); await bot.activateEntity(sheep).catch(() => {}); await h.ticks(10) }
  const wool = Object.values(bot.entities).filter(e => e.name === 'item' && e.getDroppedItem()).map(e => e.getDroppedItem())
  h.result('shear_sheep', `${[...new Set(wool.map(w => w.name))].join(',')} 1to3=${wool.reduce((a, w) => a + w.count, 0) >= 1 && wool.reduce((a, w) => a + w.count, 0) <= 3} shears_damage=${bot.heldItem ? (bot.heldItem.durabilityUsed ?? 'n/a') : 'none'}`)

  // Chickens lay the egg of their variant.
  h.cmd('kill @e[type=item]')
  h.cmd('kill @e[type=!player]')
  await h.ticks(5)
  for (const [v, dx] of [['temperate', -2], ['warm', 0], ['cold', 2]]) h.cmd(`summon chicken ${X + dx + 0.5} ${Y + 1} ${P + 0.5} {NoAI:1b,EggLayTime:20,variant:"minecraft:${v}"}`)
  await h.ticks(80)
  const eggs = Object.values(bot.entities).filter(e => e.name === 'item' && e.getDroppedItem()).map(e => e.getDroppedItem().name)
  h.result('chicken_eggs', [...new Set(eggs)].sort().join(','))
})
