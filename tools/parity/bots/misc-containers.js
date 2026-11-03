// Small containers used by hand: decorated pots taking and dropping items, lecterns taking a book
// and giving it back from the menu, chiseled bookshelves storing books by slot, and item frames.
const { make } = require('./survival-lib')
const h = make('Potter')
const { bot } = h
const X = 3300; const Y = 200; const Z = 0
const BX = X; const BY = Y + 1; const BZ = Z - 2

const place = async (block, x = BX, y = BY, z = BZ) => {
  const name = block.split('[')[0]
  for (let i = 0; i < 5; i++) {
    h.cmd(`setblock ${x} ${y} ${z} air`)
    h.cmd(`setblock ${x} ${y} ${z} ${block}`)
    if (await h.waitFor(() => h.blockName(x, y, z) === name, 2000)) break
  }
  await h.ticks(2)
}
const hold = async (item) => {
  await h.fill({ 0: item })
  bot.setQuickBarSlot(0)
  await h.ticks(3)
}
// Dropped items and whatever the bot already picked up, by total count.
const drops = () => {
  const c = {}
  for (const e of Object.values(bot.entities)) {
    const it = e.name === 'item' && e.getDroppedItem()
    if (it) c[it.name] = (c[it.name] || 0) + it.count
  }
  for (const it of bot.inventory.items()) c[it.name] = (c[it.name] || 0) + it.count
  return Object.keys(c).sort().map(k => `${k}*${c[k]}`).join(',') || 'none'
}
const props = (x = BX, y = BY, z = BZ) => {
  const p = h.block(x, y, z).getProperties()
  return Object.keys(p).sort().map(k => `${k}=${p[k]}`).join(',')
}

h.start(async () => {
  await h.arena(X, Y, Z, 6)
  h.cmd(`effect give ${bot.username} resistance infinite 255 true`)
  // The bot stands south of the blocks and faces north towards them.
  await h.tp(BX + 0.5, Y + 1, BZ + 2.5, 180, 0)

  // Decorated pot: right clicks add one item at a time, only of the same kind.
  await place('decorated_pot[facing=south]')
  await hold('emerald 5')
  for (let i = 0; i < 3; i++) { await bot.activateBlock(h.block(BX, BY, BZ)); await h.ticks(4) }
  h.result('pot.after_3_clicks', h.invStr())
  await h.fill({ 0: 'diamond 2' })
  await h.ticks(3)
  await bot.activateBlock(h.block(BX, BY, BZ))
  await h.ticks(4)
  h.result('pot.other_item', h.invStr())
  h.cmd('kill @e[type=item]')
  await h.ticks(5)
  await h.clear()
  await bot.dig(h.block(BX, BY, BZ), true)
  await h.ticks(20)
  h.result('pot.broken_drops', drops())
  h.cmd('kill @e[type=item]')

  // Lectern: a written book goes on it; taking it from the menu gives it back.
  await place('lectern[facing=south]')
  await hold('written_book[written_book_content={title:"t",author:"a",pages:["one","two"]}]')
  await bot.activateBlock(h.block(BX, BY, BZ))
  await h.ticks(5)
  h.result('lectern.placed', `${props()} inv=${h.invStr()}`)
  const lw = await bot.openBlock(h.block(BX, BY, BZ))
  await h.ticks(3)
  h.result('lectern.window', lw.type)
  bot._client.write('enchant_item', { windowId: lw.id, enchantment: 3 })
  await h.ticks(5)
  h.result('lectern.taken', `${props()} inv=${h.invStr()}`)
  if (bot.currentWindow) bot.closeWindow(bot.currentWindow)
  await h.ticks(5)

  // Chiseled bookshelf: a book clicked into the front face fills a slot and can be taken back.
  await place('chiseled_bookshelf[facing=south]')
  await hold('book 3')
  await bot.activateBlock(h.block(BX, BY, BZ), new h.Vec3(0, 0, 1), new h.Vec3(0.2, 0.8, 1))
  await h.ticks(5)
  await bot.activateBlock(h.block(BX, BY, BZ), new h.Vec3(0, 0, 1), new h.Vec3(0.8, 0.2, 1))
  await h.ticks(5)
  h.result('bookshelf.two_books', `${props()} inv=${h.invStr()}`)
  await h.fill({})
  await bot.activateBlock(h.block(BX, BY, BZ), new h.Vec3(0, 0, 1), new h.Vec3(0.2, 0.8, 1))
  await h.ticks(5)
  h.result('bookshelf.took_one', `${props()} inv=${h.invStr()}`)

  // Item frame: placed on a block face, takes the held item, then rotates it on later clicks.
  await place('stone')
  await hold('item_frame')
  await bot.activateBlock(h.block(BX, BY, BZ), new h.Vec3(0, 0, 1), new h.Vec3(0.5, 0.5, 1))
  await h.ticks(10)
  const frame = Object.values(bot.entities).find(e => e.name === 'item_frame')
  h.result('frame.placed', `${!!frame} inv=${h.invStr()}`)
  if (frame) {
    await hold('compass')
    await bot.activateEntity(frame)
    await h.ticks(5)
    await bot.activateEntity(frame)
    await h.ticks(5)
    const meta = frame.metadata
    h.result('frame.filled', `inv=${h.invStr()} rotation=${meta[meta.length - 1]}`)
  }
})
