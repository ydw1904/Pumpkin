// Hoppers: transfer rate between chests, which furnace and brewing stand slots they fill from the
// top and the side and pull from below, item entity pickup, redstone locking, shulker boxes refused
// by shulker boxes, hopper minecarts, and double chests read as one container by hoppers and
// comparators. Containers are filled with /item so the setup doesn't
// depend on block entity data in /setblock.
const { make } = require('./survival-lib')
const h = make('Hopper')
const { bot } = h
const X = 2700; const Y = 200; const Z = 0

const place = async (x, y, z, block) => {
  const name = block.split('[')[0]
  for (let i = 0; i < 5; i++) {
    h.cmd(`setblock ${x} ${y} ${z} ${block}`)
    if (await h.waitFor(() => h.blockName(x, y, z) === name, 2000)) break
  }
}
const put = (x, y, z, items) => items.forEach((it, i) => h.cmd(`item replace block ${x} ${y} ${z} container.${i} with ${it}`))
// Opens the container from next to it and lists its slots.
const contents = async (x, y, z, size) => {
  await h.tp(x + 0.5, Y + 1, z + 2.5)
  const w = await bot.openBlock(h.block(x, y, z))
  await h.ticks(2)
  await bot._syncWindow(w)
  const s = h.win(0, size === undefined ? w.slots.length - 36 : size)
  bot.closeWindow(w)
  await h.ticks(3)
  return s
}
const count = (s, name) => [...s.matchAll(new RegExp(`${name}\\*(\\d+)`, 'g'))].reduce((a, m) => a + Number(m[1]), 0)

h.start(async () => {
  await h.arena(X, Y, Z, 10)
  const y = Y + 1

  // Chest above a hopper above a chest: one item per 8 ticks, so about 10 in 81 ticks.
  let x = X - 6; let z = Z - 6
  await place(x, y + 2, z, 'chest[facing=north]')
  put(x, y + 2, z, ['cobblestone 64'])
  await place(x, y, z, 'chest[facing=north]')
  const start = bot.time.age
  await place(x, y + 1, z, 'hopper[facing=down]')
  await h.ticks(81)
  const moved = count(await contents(x, y, z), 'cobblestone')
  // Opening the chest takes a few ticks on top of the 81.
  h.result('rate.moved_in_81_ticks', moved >= 10 && moved <= 14 ? '10..14' : moved)
  void start

  // Into a furnace from the top and from the side, out of it from below.
  x = X - 2; z = Z - 6
  await place(x, y + 1, z, 'furnace[facing=north]')
  h.cmd(`item replace block ${x} ${y + 1} ${z} container.2 with iron_ingot 3`)
  await place(x, y, z, 'hopper[facing=down]')
  await place(x, y + 2, z, 'hopper[facing=down]')
  put(x, y + 2, z, ['raw_iron 2', 'coal 2'])
  await place(x + 1, y + 1, z, 'hopper[facing=west]')
  put(x + 1, y + 1, z, ['coal 2', 'raw_iron 2'])
  await h.ticks(40)
  h.result('furnace.slots', `${await contents(x, y + 1, z, 3)} top=${await contents(x, y + 2, z)} side=${await contents(x + 1, y + 1, z)} below=${await contents(x, y, z)}`)

  // Into a brewing stand from the top (ingredient) and the side (bottles and fuel).
  x = X + 2; z = Z - 6
  await place(x, y + 1, z, 'brewing_stand')
  await place(x, y + 2, z, 'hopper[facing=down]')
  put(x, y + 2, z, ['nether_wart 2', 'dirt 1'])
  await place(x + 1, y + 1, z, 'hopper[facing=west]')
  put(x + 1, y + 1, z, ['glass_bottle 1', 'blaze_powder 2', 'potion[potion_contents={potion:"minecraft:water"}] 1'])
  await h.ticks(40)
  h.result('brewing.slots', `${await contents(x, y + 1, z, 5)} top=${await contents(x, y + 2, z)} side=${await contents(x + 1, y + 1, z)}`)

  // Item entities above a hopper are pulled in.
  x = X - 6; z = Z
  await place(x, y, z, 'hopper[facing=down]')
  h.cmd(`summon item ${x + 0.5} ${y + 1.2} ${z + 0.5} {Item:{id:"minecraft:dirt",count:30},PickupDelay:32767s,NoGravity:1b}`)
  h.cmd(`summon item ${x + 0.5} ${y + 1.2} ${z + 0.5} {Item:{id:"minecraft:stone",count:3},PickupDelay:32767s,NoGravity:1b}`)
  await h.ticks(20)
  h.result('pickup.hopper', await contents(x, y, z))

  // A powered hopper doesn't move anything.
  x = X - 2; z = Z
  await place(x, y + 2, z, 'chest[facing=north]')
  put(x, y + 2, z, ['cobblestone 5'])
  await place(x, y, z, 'chest[facing=north]')
  await place(x + 1, y + 1, z, 'redstone_block')
  await place(x, y + 1, z, 'hopper[facing=down]')
  await h.ticks(30)
  h.result('locked.hopper', `${await contents(x, y, z)} hopper=${await contents(x, y + 1, z)} enabled=${h.block(x, y + 1, z).getProperties().enabled}`)

  // A hopper pushing a shulker box into a shulker box keeps it.
  x = X + 2; z = Z
  await place(x, y, z, 'shulker_box')
  await place(x - 1, y, z, 'hopper[facing=east]')
  put(x - 1, y, z, ['shulker_box 1', 'dirt 1'])
  await h.ticks(30)
  h.result('shulker.hopper', `${await contents(x, y, z)} hopper=${await contents(x - 1, y, z)}`)
  // A closed shulker box under a block can't open.
  await place(x, y + 1, z, 'stone')
  await h.tp(x + 0.5, Y + 1, z + 2.5)
  h.result('shulker.blocked_open', await bot.openBlock(h.block(x, y, z)).then((w) => { bot.closeWindow(w); return 'opened' }, () => 'refused'))
  await h.ticks(5)

  // A hopper under the left half of a blocked double chest pulls from the right half first,
  // since the right half is the first half of the combined container.
  x = X - 8; z = Z + 5
  await place(x, y + 2, z, 'chest[facing=north,type=left]')
  await place(x + 1, y + 2, z, 'chest[facing=north,type=right]')
  put(x, y + 2, z, ['dirt 1'])
  put(x + 1, y + 2, z, ['stone 1'])
  await place(x, y + 3, z, 'stone')
  await place(x + 1, y + 3, z, 'stone')
  await place(x, y + 1, z, 'hopper[facing=east]')
  await h.ticks(40)
  h.result('double_chest.pull', await contents(x, y + 1, z))

  // A hopper pushing into a full left half fills the right half.
  x = X - 4; z = Z + 5
  await place(x, y, z, 'chest[facing=north,type=left]')
  await place(x + 1, y, z, 'chest[facing=north,type=right]')
  put(x, y, z, Array(27).fill('stone 64'))
  await place(x, y + 1, z, 'hopper[facing=down]')
  put(x, y + 1, z, ['dirt 3'])
  await h.ticks(40)
  const pushed = await contents(x + 1, y, z)
  h.result('double_chest.push', `dirt=${count(pushed, 'dirt')} hopper=${await contents(x, y + 1, z)}`)

  // A comparator reads both halves, and nothing while either half is blocked.
  x = X + 1; z = Z + 5
  await place(x, y, z, 'chest[facing=north,type=left]')
  await place(x + 1, y, z, 'chest[facing=north,type=right]')
  put(x, y, z, Array(27).fill('stone 64'))
  await place(x + 1, y, z + 1, 'comparator[facing=north]')
  await place(x + 1, y, z + 2, 'redstone_wire')
  await h.ticks(10)
  const open = h.block(x + 1, y, z + 2).getProperties().power
  await place(x, y + 1, z, 'stone')
  // Changing the contents makes the comparator read again.
  put(x + 1, y, z, ['dirt 1'])
  await h.ticks(10)
  h.result('double_chest.comparator', `open=${open} blocked=${h.block(x + 1, y, z + 2).getProperties().power}`)

  // A hopper under a chest minecart empties the cart.
  x = X + 6; z = Z
  await place(x, y, z, 'hopper[facing=down]')
  await place(x, y + 1, z, 'rail')
  h.cmd(`summon chest_minecart ${x + 0.5} ${y + 1} ${z + 0.5} {Items:[{Slot:0b,id:"minecraft:dirt",count:10}]}`)
  await h.ticks(100)
  h.result('minecart.hopper_below', await contents(x, y, z))
})
