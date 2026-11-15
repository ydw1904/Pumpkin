// Container locks: a locked container only opens for a player holding a matching item in the
// main hand (or a spectator). Anyone else gets the container.isLocked action bar message and the
// chest_locked sound, a double chest needs both halves to let the player in, and hoppers ignore
// locks. Spectators can't open a container whose loot table is still pending.
const { make } = require('./survival-lib')
const h = make('Locksmith')
const { bot } = h
const X = 3900; const Y = 200; const Z = 0
const BX = X; const BY = Y + 1; const BZ = Z
const LOCK = 'lock:{components:{"minecraft:custom_name":"Key"}}'

const flat = (j) => {
  if (j === null || j === undefined) return ''
  if (typeof j !== 'object') return String(j)
  if (Object.keys(j).length === 1 && '' in j) return flat(j[''])
  if (j.translate) return j.translate + (j.with ? '(' + j.with.map(flat).join(',') + ')' : '')
  if (j.text === undefined && !j.extra) return JSON.stringify(j)
  return (j.text ?? '') + (j.extra ? j.extra.map(flat).join('') : '')
}
// Raw NBT text components, because prismarine-chat drops arguments.
const plain = (t) => {
  if (t === null || typeof t !== 'object') return t
  if (t.type === 'compound') return Object.fromEntries(Object.entries(t.value).map(([k, v]) => [k, plain(v)]))
  if (t.type === 'list') return (t.value.value || []).map(v => plain({ type: t.value.type, value: v }))
  if (t.type !== undefined && 'value' in t) return plain(t.value)
  return t
}
const bar = []
bot._client.on('system_chat', (p) => { if (p.isActionBar) bar.push(flat(plain(p.content))) })
const heard = []
bot._client.on('sound_effect', (p) => {
  heard.push(`${p.sound && p.sound.soundId}/${p.soundCategory}@${p.x},${p.y},${p.z}`)
})

// Right-clicks the block and reports whether a window opened (and its container size), the
// action bar messages and the sounds that came with it.
const tryOpen = async (x, y, z) => {
  bar.length = 0
  heard.length = 0
  await bot.activateBlock(h.block(x, y, z)).catch(() => {})
  await h.waitFor(() => bot.currentWindow, 1500)
  await h.ticks(4)
  const w = bot.currentWindow
  const sounds = heard.join('|') || 'none'
  let res = 'refused'
  if (w) {
    res = `open:${w.inventoryStart}`
    bot.closeWindow(w)
    await h.ticks(4)
  }
  return `${res} bar=${bar.join('|') || 'none'} sound=${sounds}`
}
const set = async (x, y, z, spec) => {
  const name = spec.split(/[[{]/)[0]
  h.cmd(`setblock ${x} ${y} ${z} air`)
  await h.waitFor(() => h.blockName(x, y, z) === 'air', 3000)
  h.cmd(`setblock ${x} ${y} ${z} ${spec}`)
  await h.waitFor(() => h.blockName(x, y, z) === name, 3000)
  await h.ticks(2)
}
const hold = async (item, n = 1) => {
  await h.clear()
  if (!item) return
  await h.give(item, n)
  const it = bot.inventory.items().find((i) => i.name === item.split('[')[0])
  if (it) await bot.equip(it, 'hand').catch(() => {})
  await h.ticks(2)
}

h.start(async () => {
  await h.arena(X, Y, Z, 6)
  h.cmd(`effect give ${bot.username} resistance infinite 255 true`)
  await h.tp(BX + 0.5, Y + 1, BZ + 2.5, 180, 0)

  // Every block entity with a lock, without and with the key.
  for (const block of ['chest', 'trapped_chest', 'barrel', 'shulker_box', 'furnace', 'blast_furnace', 'smoker',
    'hopper', 'dispenser', 'dropper', 'crafter', 'brewing_stand', 'beacon']) {
    await set(BX, BY, BZ, `${block}{${LOCK}}`)
    await hold(null)
    h.result(`${block}.no_key`, await tryOpen(BX, BY, BZ))
    await hold('stick[custom_name="Key"]')
    h.result(`${block}.key`, await tryOpen(BX, BY, BZ))
  }

  // Only the main hand counts, and the item has to match the predicate.
  await set(BX, BY, BZ, `chest{${LOCK}}`)
  await hold('stick[custom_name="Nope"]')
  h.result('chest.wrong_name', await tryOpen(BX, BY, BZ))
  await hold('diamond')
  h.cmd(`item replace entity ${bot.username} weapon.offhand with stick[custom_name="Key"]`)
  await h.waitFor(() => bot.inventory.slots[45], 3000)
  h.result('chest.offhand_key', await tryOpen(BX, BY, BZ))
  await set(BX, BY, BZ, 'barrel{lock:{items:"minecraft:diamond",count:{min:2}}}')
  await hold('diamond', 1)
  h.result('barrel.one_diamond', await tryOpen(BX, BY, BZ))
  await hold('diamond', 2)
  h.result('barrel.two_diamonds', await tryOpen(BX, BY, BZ))

  // The message names the container by its custom name.
  await set(BX, BY, BZ, `chest{CustomName:"Treasure",${LOCK}}`)
  await hold(null)
  h.result('chest.named', await tryOpen(BX, BY, BZ))

  // A double chest with one locked half: both halves refuse, the message names the double chest
  // and the sound plays between the halves.
  h.cmd(`setblock ${BX} ${BY} ${BZ} air`)
  await set(BX + 1, BY, BZ, 'chest[facing=north,type=right]')
  await set(BX, BY, BZ, `chest[facing=north,type=left]{${LOCK}}`)
  await hold(null)
  h.result('double.left_locked.open_right', await tryOpen(BX + 1, BY, BZ))
  h.result('double.left_locked.open_left', await tryOpen(BX, BY, BZ))
  await hold('stick[custom_name="Key"]')
  h.result('double.left_locked.key', await tryOpen(BX + 1, BY, BZ))
  await set(BX, BY, BZ, 'chest[facing=north,type=left]')
  await set(BX + 1, BY, BZ, `chest[facing=north,type=right]{CustomName:"Treasure",${LOCK}}`)
  await hold(null)
  h.result('double.right_locked.open_left', await tryOpen(BX, BY, BZ))
  h.cmd(`setblock ${BX + 1} ${BY} ${BZ} air`)
  h.cmd(`setblock ${BX} ${BY} ${BZ} air`)
  await h.waitFor(() => h.blockName(BX, BY, BZ) === 'air', 3000)

  // Hoppers move items out of and into locked containers.
  await set(BX, BY, BZ, 'chest')
  await set(BX, BY + 1, BZ, 'hopper[facing=down]')
  await set(BX, BY + 2, BZ, `chest{${LOCK},Items:[{Slot:0b,id:"minecraft:diamond",count:3}]}`)
  await h.serverTicks(60)
  await hold(null)
  const w = await bot.openBlock(h.block(BX, BY, BZ)).catch(() => null)
  h.result('hopper.pulled', w ? (w.slots.slice(0, w.inventoryStart).filter(Boolean).map(h.stackStr).join(',') || 'empty') : 'refused')
  if (w) { bot.closeWindow(w); await h.ticks(2) }
  h.cmd(`fill ${BX} ${BY} ${BZ} ${BX} ${BY + 2} ${BZ} air`)
  await h.waitFor(() => h.blockName(BX, BY + 2, BZ) === 'air', 3000)

  // Spectators ignore locks but can't open a container with a pending loot table.
  h.cmd(`gamemode spectator ${bot.username}`)
  await h.waitFor(() => bot.game.gameMode === 'spectator', 5000)
  await h.ticks(5)
  await set(BX, BY, BZ, `chest{${LOCK}}`)
  h.result('spectator.locked_chest', await tryOpen(BX, BY, BZ))
  await set(BX, BY, BZ, `barrel{${LOCK}}`)
  h.result('spectator.locked_barrel', await tryOpen(BX, BY, BZ))
  await set(BX, BY, BZ, 'barrel{LootTable:"minecraft:chests/simple_dungeon"}')
  h.result('spectator.loot_barrel', await tryOpen(BX, BY, BZ))
  await set(BX, BY, BZ, `chest{LootTable:"minecraft:chests/simple_dungeon",${LOCK}}`)
  h.result('spectator.loot_locked_chest', await tryOpen(BX, BY, BZ))
  h.cmd(`gamemode survival ${bot.username}`)
  await h.waitFor(() => bot.game.gameMode === 'survival', 5000)
  h.cmd(`setblock ${BX} ${BY} ${BZ} air`)
  await h.ticks(5)
})
