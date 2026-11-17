// Editing and signing a book in the slot the packet names: a hotbar slot or 40 for the offhand.
// Signing keeps the book's other components; slots outside the hotbar are ignored.
const { make } = require('./survival-lib')
const h = make('Scribe')
const { bot } = h
const X = 2700; const Y = 200; const Z = 0

const describe = (it) => {
  if (!it) return 'empty'
  const types = (it.components || []).map(c => c.type).sort().join('+')
  return `${it.name}*${it.count}[${types}]`
}
// Window slots: 36-44 hotbar, 9-35 main, 45 offhand.
const show = () => `main=${describe(bot.inventory.slots[36])} hot1=${describe(bot.inventory.slots[37])} inv9=${describe(bot.inventory.slots[9])} off=${describe(bot.inventory.slots[45])}`
const edit = async (hand, pages, title) => {
  bot._client.write('edit_book', { hand, pages, title })
  await h.ticks(10)
}

h.start(async () => {
  await h.arena(X, Y, Z, 4)
  await h.clear()
  h.cmd(`item replace entity ${bot.username} hotbar.0 with writable_book`)
  h.cmd(`item replace entity ${bot.username} hotbar.1 with writable_book[custom_name="Notes"]`)
  h.cmd(`item replace entity ${bot.username} inventory.0 with writable_book`)
  h.cmd(`item replace entity ${bot.username} weapon.offhand with writable_book[custom_name="Notes"]`)
  await h.waitFor(() => bot.inventory.slots[45] && bot.inventory.slots[9] && bot.inventory.slots[37], 5000)
  await h.ticks(5)
  h.result('start', show())

  await edit(40, ['offhand page'])
  h.result('edit_offhand', show())
  await edit(1, ['hotbar page'])
  h.result('edit_hotbar1', show())
  await edit(40, ['signed page', 'two'], 'Title')
  h.result('sign_offhand', show())
  await edit(1, ['signed hotbar'], 'Other')
  h.result('sign_hotbar1', show())
  // Inventory slot 9 is not a hotbar slot.
  await edit(9, ['nope'], 'Nope')
  h.result('sign_inventory9', show())
  // A book that is no longer writable can't be signed again.
  await edit(40, ['again'], 'Again')
  h.result('sign_again', show())
})
