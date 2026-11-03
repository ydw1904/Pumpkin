// Clicks in the player inventory: shift-click routing, number keys and offhand swap, double-click
// collect, left and right drags, dropping, the 2x2 crafting grid with remainders and shift-crafting,
// and creative middle-click. Each click is answered with the whole window, so the RESULT lines show
// the server's outcome.
const { make } = require('./survival-lib')
const h = make('Clicker')
const { bot } = h
const X = 2100; const Y = 200; const Z = 0

// Dropped items by total count; whether thrown stacks have merged yet depends on timing.
const drops = () => {
  const c = {}
  for (const e of Object.values(bot.entities)) {
    const it = e.name === 'item' && e.getDroppedItem()
    if (it) c[it.name] = (c[it.name] || 0) + it.count
  }
  return Object.keys(c).sort().map(k => `${k}*${c[k]}`).join(',') || 'none'
}
const killDrops = async () => { h.cmd('kill @e[type=item]'); await h.waitFor(() => drops() === 'none', 3000) }

h.start(async () => {
  await h.arena(X, Y, Z, 6)
  const step = async (key, map, clicks) => {
    await h.fill(map)
    for (const c of clicks) await h.click(...c)
    h.result(key, h.win())
  }

  await step('shift_hotbar_to_main', { 0: 'cobblestone 64' }, [[36, 0, 1]])
  await step('shift_main_to_hotbar', { 9: 'cobblestone 64' }, [[9, 0, 1]])
  await step('shift_merges', { 0: 'cobblestone 30', 10: 'cobblestone 50', 20: 'cobblestone 60' }, [[36, 0, 1]])
  await step('shift_armor_in', { 9: 'iron_helmet', 10: 'diamond_chestplate', 11: 'shield', 12: 'elytra', 13: 'carved_pumpkin', 14: 'turtle_helmet', 15: 'iron_boots' },
    [[9, 0, 1], [10, 0, 1], [11, 0, 1], [12, 0, 1], [13, 0, 1], [14, 0, 1], [15, 0, 1]])
  await h.click(5, 0, 1)
  await h.click(8, 0, 1)
  h.result('shift_armor_out', h.win())
  await step('shift_offhand_out', { 0: 'dirt 3' }, [[36, 40, 2], [45, 0, 1]])
  await step('shift_full_inventory', Object.fromEntries([...Array(36).keys()].map(i => [i, i === 0 ? 'dirt 1' : 'stone 64'])), [[36, 0, 1]])

  await step('number_key_swap', { 9: 'dirt 5', 1: 'stone 3' }, [[9, 1, 2]])
  await step('number_key_offhand', { 9: 'dirt 5' }, [[9, 40, 2]])
  await step('number_key_armor', { 0: 'iron_boots', 1: 'dirt 4' }, [[8, 0, 2], [5, 1, 2]])
  await step('number_key_same_hotbar', { 0: 'dirt 5', 2: 'stone 1' }, [[36, 2, 2]])

  await step('double_click', { 9: 'dirt 10', 10: 'dirt 20', 11: 'dirt 64', 12: 'dirt 63', 36: 'dirt 5' }, [[9, 0, 0], [9, 0, 6]])
  await step('double_click_no_room', { 9: 'dirt 64', 10: 'dirt 5' }, [[10, 0, 0], [10, 0, 6]])

  const drag = (start, add, end, slots) => [[-999, start, 5], ...slots.map(s => [s, add, 5]), [-999, end, 5]]
  await step('drag_left', { 9: 'dirt 10' }, [[9, 0, 0], ...drag(0, 1, 2, [10, 11, 12])])
  await step('drag_right', { 9: 'dirt 10' }, [[9, 0, 0], ...drag(4, 5, 6, [10, 11, 12])])
  await step('drag_left_onto_stacks', { 9: 'dirt 11', 10: 'dirt 62', 11: 'stone 1' }, [[9, 0, 0], ...drag(0, 1, 2, [10, 11, 12, 13])])
  await step('drag_single_slot', { 9: 'dirt 10' }, [[9, 0, 0], ...drag(0, 1, 2, [12])])
  await step('right_click_split', { 9: 'dirt 11' }, [[9, 1, 0], [10, 1, 0], [11, 0, 0]])
  await step('left_click_swap', { 9: 'dirt 11', 10: 'stone 3' }, [[9, 0, 0], [10, 0, 0]])

  await killDrops()
  await step('drop_one', { 0: 'dirt 10' }, [[36, 0, 4]])
  await step('drop_stack', { 0: 'dirt 10' }, [[36, 1, 4]])
  await h.ticks(10)
  h.result('drops', drops())
  await killDrops()
  await step('cursor_drop', { 0: 'dirt 10' }, [[36, 0, 0], [-999, 1, 0], [-999, 0, 0]])
  await h.ticks(10)
  h.result('cursor_drops', drops())
  await killDrops()

  await step('craft_grid_place', { 9: 'oak_log 5' }, [[9, 0, 0], [1, 1, 0], [9, 0, 0]])
  await h.click(0, 0, 1)
  h.result('craft_shift_one', h.win())
  await step('craft_shift_many', { 9: 'oak_log 5' }, [[9, 0, 0], [1, 0, 0], [0, 0, 1]])
  await step('craft_pickup_result', { 9: 'oak_log 5' }, [[9, 0, 0], [1, 0, 0], [0, 0, 0], [0, 0, 0], [0, 1, 0]])
  await step('craft_remainder', { 9: 'honey_bottle 3' }, [[9, 0, 0], [1, 0, 0], [0, 0, 1]])
  await step('craft_remainder_single', { 9: 'honey_bottle 1' }, [[9, 0, 0], [1, 0, 0], [0, 0, 0]])
  await step('craft_shaped_2x2', { 9: 'oak_planks 8' }, [[9, 0, 0], [1, 1, 0], [2, 1, 0], [3, 1, 0], [4, 1, 0], [9, 0, 0], [0, 0, 1]])
  await step('craft_full_inventory', Object.fromEntries([...Array(36).keys()].map(i => [i, i === 0 ? 'oak_log 2' : 'stone 64'])), [[36, 0, 0], [1, 0, 0], [0, 0, 1]])
  await step('craft_number_key', { 9: 'oak_log 2', 1: 'oak_planks 60' }, [[9, 0, 0], [1, 0, 0], [0, 1, 2]])
  await step('craft_drop_result', { 9: 'oak_log 2' }, [[9, 0, 0], [1, 0, 0], [0, 1, 4]])
  await killDrops()

  // Closing the inventory returns the grid and the cursor to the inventory.
  await h.fill({ 9: 'oak_log 5', 10: 'dirt 3' })
  await h.click(9, 0, 0); await h.click(1, 1, 0); await h.click(2, 1, 0); await h.click(10, 0, 0)
  bot._client.write('close_window', { windowId: 0 })
  await h.ticks(10)
  await bot._syncWindow(bot.inventory)
  h.result('close_returns_grid', h.win())
  await h.ticks(10)
  h.result('close_drops', drops())
  await killDrops()

  // Creative: middle-click clones a full stack, survival can't.
  await step('middle_click_survival', { 9: 'dirt 3' }, [[9, 2, 3]])
  h.cmd(`gamemode creative ${bot.username}`)
  await h.waitFor(() => bot.game.gameMode === 'creative', 5000)
  await step('middle_click_creative', { 9: 'dirt 3' }, [[9, 2, 3]])
  h.cmd(`gamemode survival ${bot.username}`)
  await h.waitFor(() => bot.game.gameMode === 'survival', 5000)
})
