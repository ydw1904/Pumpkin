// Dispenser and dropper behaviours: what one pulse does with each item, what is left in the
// dispenser, which block ends up in front and which entities appear.
const { make } = require('./survival-lib')
const h = make('Dispenser')
const { bot } = h
const X = 2900; const Y = 200; const Z = 0

const place = async (x, y, z, block) => {
  const name = block.split('[')[0].split('{')[0]
  for (let i = 0; i < 5; i++) {
    h.cmd(`setblock ${x} ${y} ${z} ${block}`)
    if (await h.waitFor(() => h.blockName(x, y, z) === name, 2000)) break
  }
}
const contents = async (x, y, z) => {
  const back = bot.entity.position.clone()
  await h.tp(x + 0.5, Y + 1, z + 1.5)
  const w = await bot.openBlock(h.block(x, y, z))
  await h.ticks(2)
  await bot._syncWindow(w)
  const s = h.win(0, w.slots.length - 36)
  bot.closeWindow(w)
  await h.ticks(3)
  await h.tp(back.x, back.y, back.z)
  return s
}
// The pig from the spawn egg lane may still be dying in the next lane.
const near = (x, y, z) => Object.values(bot.entities).filter(e => e !== bot.entity && e.name !== 'pig' && e.name !== 'chicken' && e.position.distanceTo(new h.Vec3(x + 0.5, y + 0.5, z + 0.5)) < 6)
  .map(e => e.name === 'item' && e.getDroppedItem() ? `item:${h.stackStr(e.getDroppedItem())}` : e.name).filter(n => !n.startsWith('item:porkchop')).sort().join(',') || 'none'

// A dispenser at (x, y, z) facing east, holding `items`, fires once; `front` is placed first.
let lane = 0
const fire = async (key, items, opts = {}) => {
  const block = opts.dropper ? 'dropper' : 'dispenser'
  // Lanes on a 5 x 7 grid, four blocks apart in x and three in z.
  const x = X - 9 + 4 * (lane % 5); const y = Y + 1; const z = Z - 9 + 3 * Math.floor(lane / 5)
  lane++
  h.cmd(`kill @e[type=!player,distance=..40,x=${X},y=${Y},z=${Z}]`)
  if (opts.below) await place(x + 1, y - 1, z, opts.below)
  if (opts.front) await place(x + 1, y, z, opts.front)
  if (opts.target) await place(x + 2, y, z, opts.target)
  await place(x, y, z, `${block}[facing=${opts.facing || 'east'}]`)
  items.forEach((it, i) => h.cmd(`item replace block ${x} ${y} ${z} container.${i} with ${it[0]}${it[2] ? `[${it[2]}]` : ''} ${it[1]}`))
  await h.ticks(2)
  await place(x - 1, y, z, 'redstone_block')
  await h.ticks(opts.wait || 10)
  const front = h.block(x + 1, y, z)
  // Fast or short-lived entities may already be gone; their lanes only report what was used up.
  const seen = `front=${front ? front.name : 'unloaded'}` + (opts.projectile ? '' : ` entities=${near(x + 1, y, z)}`)
  // Clear placed fluids and fire before they reach other lanes.
  if (!opts.keepFront) {
    h.cmd(`fill ${x + 1} ${y} ${z - 1} ${x + 3} ${y + 1} ${z + 1} air`)
    h.cmd(`fill ${x + 1} ${Y} ${z - 1} ${x + 3} ${Y} ${z + 1} stone`)
  }
  h.result(key, `left=${await contents(x, y, z)} ${seen}`)
  return { x, y, z }
}

h.start(async () => {
  await h.arena(X, Y, Z, 12)
  h.cmd(`effect give ${bot.username} resistance infinite 255 true`)
  h.cmd(`tp ${bot.username} ${X + 11} ${Y + 1} ${Z + 11}`)
  await h.ticks(10)

  await fire('arrow', [['arrow', 3]], { wait: 3, projectile: true })
  await fire('cobblestone', [['cobblestone', 3]])
  await fire('dropper_cobblestone', [['cobblestone', 3]], { dropper: true })
  const d = await fire('dropper_into_chest', [['cobblestone', 3]], { dropper: true, front: 'chest[facing=north]', keepFront: true })
  h.result('dropper_into_chest.chest', await contents(d.x + 1, d.y, d.z))
  await fire('water_bucket', [['water_bucket', 1]])
  await fire('lava_bucket', [['lava_bucket', 1]])
  await fire('empty_bucket_water', [['bucket', 2]], { front: 'water' })
  await fire('empty_bucket_air', [['bucket', 2]])
  await fire('powder_snow_bucket', [['powder_snow_bucket', 1]])
  await fire('shulker_box', [['shulker_box', 1]])
  await fire('flint_and_steel', [['flint_and_steel', 1]])
  await fire('flint_and_steel_tnt', [['flint_and_steel', 1]], { front: 'tnt' })
  await fire('bone_meal_sapling', [['bone_meal', 3]], { front: 'oak_sapling', below: 'grass_block' })
  await fire('bone_meal_air', [['bone_meal', 3]])
  await fire('tnt', [['tnt', 2]], { wait: 3, projectile: true })
  await fire('fire_charge', [['fire_charge', 2]], { wait: 3, projectile: true })
  await fire('snowball', [['snowball', 2]], { wait: 3, projectile: true })
  await fire('egg', [['egg', 2]], { wait: 3, projectile: true })
  await fire('potion_splash', [['splash_potion', 1, 'potion_contents={potion:"minecraft:healing"}']], { wait: 3, projectile: true })
  await fire('experience_bottle', [['experience_bottle', 1]], { wait: 3, projectile: true })
  await fire('minecart_rail', [['minecart', 1]], { front: 'rail' })
  await fire('minecart_no_rail', [['minecart', 1]])
  await fire('boat_water', [['oak_boat', 1]], { front: 'water' })
  await fire('spawn_egg', [['pig_spawn_egg', 2]])
  await fire('armor_stand', [['armor_stand', 2]])
  await fire('glass_bottle_water', [['glass_bottle', 2]], { front: 'water' })
  await fire('carved_pumpkin_golem', [['carved_pumpkin', 1]], { front: 'air', below: 'iron_block' })
  await fire('honeycomb_copper', [['honeycomb', 2]], { front: 'copper_block' })
  await fire('shears_sheep', [['shears', 1]], { wait: 5 })
  await fire('trident', [['trident', 1]], { wait: 3, projectile: true })
  await fire('firework', [['firework_rocket', 1]], { wait: 3, projectile: true })
  await fire('wind_charge', [['wind_charge', 2]], { wait: 3, projectile: true })
  await fire('dispenser_up_into_nothing', [['dirt', 2]], { facing: 'up' })

  // Armor goes onto a player standing in front.
  await h.clear()
  const x = X - 11; const y = Y + 1; const z = Z + 11
  h.cmd(`tp ${bot.username} ${x + 1.5} ${y} ${z + 0.5}`)
  await h.waitFor(() => bot.entity.position.distanceTo(new h.Vec3(x + 1.5, y, z + 0.5)) < 0.5, 5000)
  await h.ticks(10)
  await place(x, y, z, 'dispenser[facing=east]')
  h.cmd(`item replace block ${x} ${y} ${z} container.0 with iron_chestplate`)
  await place(x - 1, y, z, 'redstone_block')
  await h.ticks(10)
  h.result('armor_equip', `left=${await contents(x, y, z)} slots=${h.slots()}`)
})
