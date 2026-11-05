// Redstone a player builds, beyond the GameTests in tools/parity/tests.py: comparators reading
// containers and blocks, subtract mode, repeater locking, observers, note blocks, daylight
// detectors, target blocks, TNT, detector and powered rails, locked hoppers and slime blocks.
const { make } = require('./survival-lib')
const h = make('Wirer')
const { bot, Vec3 } = h
const X = 6000; const Y = 200; const Z = 0
// Time packets arrive every 20 ticks; two intervals leave time for queued commands to run.
const SETTLE_TICKS = 40

const props = (x, y, z) => { const b = h.block(x, y, z); return b ? b.getProperties() : {} }
const power = (x, y, z) => props(x, y, z).power
function requireScene (condition, message) {
  if (!condition) throw new Error(`Invalid redstone scene: ${message}`)
}
// Watches a block for the states it passes through.
function watch (x, y, z, key) {
  const seen = []
  const fn = (oldB, newB) => {
    if (newB && newB.position.x === x && newB.position.y === y && newB.position.z === z) {
      const v = key ? newB.getProperties()[key] : newB.name
      if (seen[seen.length - 1] !== v) seen.push(v)
    }
  }
  bot.on('blockUpdate', fn)
  return { stop: () => { bot.removeListener('blockUpdate', fn); return seen.join('>') || 'none' } }
}

h.start(async () => {
  await h.arena(X, Y, Z, 16)
  h.cmd('time set 6000')
  h.cmd('weather clear')
  const y = Y + 1

  // Comparator outputs: source block at z, comparator at z+1 facing north, wire at z+2.
  // Containers are filled with /item so the check doesn't depend on /setblock reading NBT.
  const stacks = (n, item = 'stone', count = 64) => [...Array(n).keys()].map(i => `item replace block ~ ~ ~ container.${i} with ${item} ${count}`)
  const SOURCES = [
    ['chest_empty', 'chest[facing=south]', []],
    ['chest_1_stone', 'chest[facing=south]', stacks(1, 'stone', 1)],
    ['chest_9_stacks', 'chest[facing=south]', stacks(9)],
    ['chest_full', 'chest[facing=south]', stacks(27)],
    ['hopper_1_sword', 'hopper[facing=south]', stacks(1, 'diamond_sword', 1)],
    ['hopper_8_pearls', 'hopper[facing=south]', stacks(1, 'ender_pearl', 8)],
    ['composter_3', 'composter[level=3]', []],
    ['cake_bites_2', 'cake[bites=2]', []],
    ['cauldron_2', 'water_cauldron[level=2]', []],
    ['frame_eye', 'end_portal_frame[eye=true]', []],
    ['anchor_2', 'respawn_anchor[charges=2]', []],
    ['beehive_3', 'beehive[honey_level=3]', []],
    ['furnace_half', 'furnace', stacks(1, 'iron_ore', 32)],
    ['barrel_half', 'barrel', stacks(13)]
  ]
  SOURCES.forEach(([, block, fill], i) => {
    const x = X - 14 + i * 2
    h.cmd(`setblock ${x} ${y} ${Z - 14} ${block}`)
    for (const c of fill) h.cmd(`execute positioned ${x} ${y} ${Z - 14} run ${c}`)
    h.cmd(`setblock ${x} ${y} ${Z - 13} comparator[facing=north]`)
    h.cmd(`setblock ${x} ${y} ${Z - 12} redstone_wire`)
  })
  // Subtract mode: rear 15, side 6.
  h.cmd(`setblock ${X + 14} ${y} ${Z - 14} redstone_block`)
  h.cmd(`setblock ${X + 14} ${y} ${Z - 13} comparator[facing=north,mode=subtract]`)
  h.cmd(`setblock ${X + 14} ${y} ${Z - 12} redstone_wire`)
  h.cmd(`setblock ${X + 15} ${y} ${Z - 13} redstone_wire`)
  h.cmd(`fill ${X + 16} ${y} ${Z - 13} ${X + 16} ${y} ${Z - 4} redstone_wire`)
  h.cmd(`setblock ${X + 16} ${y} ${Z - 3} redstone_block`)
  await h.tp(X + 0.5, Y + 1, Z - 9.5)
  await h.serverTicks(SETTLE_TICKS)
  SOURCES.forEach(([name], i) => h.result(`comparator_${name}`, power(X - 14 + i * 2, y, Z - 12)))
  h.result('comparator_subtract', power(X + 14, y, Z - 12))

  // A repeater locked by a powered side repeater keeps its output when its input changes.
  h.cmd(`setblock ${X - 14} ${y} ${Z - 8} lever[face=floor,powered=false]`)
  h.cmd(`setblock ${X - 14} ${y} ${Z - 7} repeater[facing=north]`)
  h.cmd(`setblock ${X - 14} ${y} ${Z - 6} redstone_lamp`)
  h.cmd(`setblock ${X - 13} ${y} ${Z - 7} repeater[facing=east]`)
  // Power arrives after placement so the side repeater receives a neighbour update.
  h.cmd(`setblock ${X - 12} ${y} ${Z - 7} redstone_block`)
  await h.serverTicks(SETTLE_TICKS)
  requireScene(props(X - 13, y, Z - 7).powered === true && props(X - 14, y, Z - 7).locked === true, 'side repeater must lock the main repeater')
  h.cmd(`setblock ${X - 14} ${y} ${Z - 8} lever[face=floor,powered=true]`)
  await h.serverTicks(SETTLE_TICKS)
  h.result('repeater_locked', `${props(X - 14, y, Z - 7).locked} lamp=${props(X - 14, y, Z - 6).lit}`)
  h.cmd(`setblock ${X - 12} ${y} ${Z - 7} air`)
  await h.serverTicks(SETTLE_TICKS)
  h.result('repeater_unlocked', `${props(X - 14, y, Z - 7).locked} lamp=${props(X - 14, y, Z - 6).lit}`)

  // Observer: pulses once when the watched block changes.
  h.cmd(`setblock ${X - 8} ${y} ${Z - 7} observer[facing=north]`)
  h.cmd(`setblock ${X - 8} ${y} ${Z - 6} redstone_lamp`)
  h.cmd(`setblock ${X - 8} ${y} ${Z - 8} stone`)
  await h.ticks(10)
  const obs = watch(X - 8, y, Z - 7, 'powered')
  h.cmd(`setblock ${X - 8} ${y} ${Z - 8} oak_planks`)
  await h.ticks(10)
  h.result('observer_pulse', obs.stop())

  // Note blocks: instrument from the block below, using raises the note, power plays it.
  const sounds = []
  bot.on('soundEffectHeard', (name, pos, vol, pitch) => { if (String(name).includes('note_block')) sounds.push(`${name.replace('minecraft:', '').replace('block.note_block.', '')}@${pitch.toFixed(2)}`) })
  const NOTES = [['stone', 'basedrum'], ['sand', 'snare'], ['glass', 'hat'], ['oak_planks', 'bass'], ['gold_block', 'bell'], ['clay', 'flute'], ['packed_ice', 'chime'], ['white_wool', 'guitar'], ['bone_block', 'xylophone'], ['iron_block', 'iron_xylophone'], ['soul_sand', 'cow_bell'], ['pumpkin', 'didgeridoo'], ['emerald_block', 'bit'], ['hay_block', 'banjo'], ['glowstone', 'pling'], ['dirt', 'harp']]
  // The block below goes in after the note block so the note block's own update picks the
  // instrument, not /setblock.
  NOTES.forEach(([below], i) => {
    h.cmd(`setblock ${X - 14 + i * 2} ${y} ${Z - 2} note_block`)
    h.cmd(`setblock ${X - 14 + i * 2} ${Y} ${Z - 2} ${below}`)
  })
  await h.ticks(10)
  h.result('note_instruments', NOTES.map(([, inst], i) => props(X - 14 + i * 2, y, Z - 2).instrument === inst ? 'ok' : `${inst}!=${props(X - 14 + i * 2, y, Z - 2).instrument}`).join(','))
  await h.tp(X - 13.5, Y + 1, Z - 0.5)
  await bot.lookAt(new Vec3(X - 13.5, y + 0.5, Z - 1.5), true)
  await bot.activateBlock(h.block(X - 14, y, Z - 2), new Vec3(0, 0, 1)).catch(() => {})
  await h.ticks(10)
  h.result('note_after_use', props(X - 14, y, Z - 2).note)
  h.cmd(`setblock ${X - 14} ${y} ${Z - 1} redstone_block`)
  h.cmd(`setblock ${X - 12} ${y + 1} ${Z - 2} stone`)
  h.cmd(`setblock ${X - 12} ${y} ${Z - 1} redstone_block`)
  h.cmd(`setblock ${X - 10} ${y + 1} ${Z - 2} zombie_head`)
  h.cmd(`setblock ${X - 10} ${y} ${Z - 1} redstone_block`)
  await h.ticks(20)
  h.result('note_sounds', sounds.join(',') || 'none')
  h.result('note_powered', `${props(X - 14, y, Z - 2).powered},${props(X - 12, y, Z - 2).powered}`)

  // Daylight detectors at several times of day.
  h.cmd(`setblock ${X} ${y} ${Z + 2} daylight_detector`)
  h.cmd(`setblock ${X + 2} ${y} ${Z + 2} daylight_detector[inverted=true]`)
  // Time stands still so the detectors' 20 tick update phase can't matter.
  h.cmd('gamerule advance_time false')
  const dl = []
  for (const t of [0, 3000, 6000, 12000, 13000, 18000, 23000]) {
    h.cmd(`time set ${t}`)
    await h.ticks(45)
    dl.push(`${t}:${props(X, y, Z + 2).power}/${props(X + 2, y, Z + 2).power}`)
  }
  h.result('daylight', dl.join(','))
  h.cmd('weather thunder')
  h.cmd('time set 6000')
  await h.ticks(160)
  h.result('daylight_thunder', `${props(X, y, Z + 2).power}/${props(X + 2, y, Z + 2).power}`)
  h.cmd('weather clear')
  h.cmd('gamerule advance_time true')

  // Target block hit dead centre from above by an arrow.
  h.cmd(`setblock ${X + 6} ${y} ${Z + 2} target`)
  await h.ticks(5)
  const tgt = watch(X + 6, y, Z + 2, 'power')
  h.cmd(`summon arrow ${X + 6.5} ${y + 4} ${Z + 2.5} {Motion:[0.0,-1.5,0.0]}`)
  await h.ticks(40)
  h.result('target_arrow', tgt.stop())

  // TNT lit by a redstone block: the block becomes an entity that explodes after 80 ticks.
  h.cmd(`fill ${X + 9} ${Y - 3} ${Z + 6} ${X + 13} ${Y} ${Z + 10} obsidian`)
  h.cmd(`setblock ${X + 11} ${y} ${Z + 8} tnt`)
  await h.ticks(5)
  h.cmd(`setblock ${X + 11} ${y} ${Z + 9} redstone_block`)
  await h.waitFor(() => Object.values(bot.entities).some(e => e.name === 'tnt'), 3000)
  const lit = bot.time.age
  h.result('tnt_primed', `${h.blockName(X + 11, y, Z + 8)} entity=${Object.values(bot.entities).some(e => e.name === 'tnt')}`)
  await h.waitFor(() => !Object.values(bot.entities).some(e => e.name === 'tnt'), 8000)
  const fuse = Number(bot.time.age - lit)
  h.result('tnt_fuse_78_to_82', fuse >= 78 && fuse <= 82)
  h.result('tnt_obsidian_intact', h.blockName(X + 11, Y, Z + 8))

  // Detector rail and a minecart; powered rail line pushing a cart off a block.
  h.cmd(`setblock ${X - 10} ${y} ${Z + 6} detector_rail`)
  h.cmd(`setblock ${X - 10} ${y} ${Z + 7} redstone_lamp`)
  await h.ticks(5)
  h.cmd(`summon minecart ${X - 9.5} ${y} ${Z + 6.5}`)
  await h.ticks(10)
  h.result('detector_rail', `${props(X - 10, y, Z + 6).powered} lamp=${props(X - 10, y, Z + 7).lit}`)
  h.cmd('kill @e[type=minecart]')
  await h.ticks(30)
  h.result('detector_rail_after', props(X - 10, y, Z + 6).powered)
  h.cmd(`setblock ${X - 15} ${y} ${Z + 10} stone`)
  h.cmd(`fill ${X - 14} ${y} ${Z + 10} ${X - 4} ${y} ${Z + 10} powered_rail[shape=east_west]`)
  h.cmd(`fill ${X - 14} ${Y} ${Z + 10} ${X - 4} ${Y} ${Z + 10} redstone_block`)
  h.cmd(`fill ${X - 3} ${y} ${Z + 10} ${X + 8} ${y} ${Z + 10} rail[shape=east_west]`)
  await h.serverTicks(SETTLE_TICKS)
  requireScene(props(X - 14, y, Z + 10).powered === true, 'rail under the minecart must be powered')
  h.cmd(`summon minecart ${X - 13.5} ${y} ${Z + 10.5}`)
  await h.serverTicks(SETTLE_TICKS)
  const cart = Object.values(bot.entities).find(e => e.name === 'minecart')
  h.result('powered_rail_cart_moved_east', cart ? cart.position.x > X - 10 : 'none')

  // A powered hopper keeps its items; the comparators read the chests below.
  for (const [x, powered] of [[X + 4, true], [X + 8, false]]) {
    h.cmd(`setblock ${x} ${y} ${Z + 12} chest`)
    h.cmd(`setblock ${x} ${y + 1} ${Z + 12} hopper[facing=down]`)
    h.cmd(`item replace block ${x} ${y + 1} ${Z + 12} container.0 with stone 5`)
    if (powered) h.cmd(`setblock ${x + 1} ${y + 1} ${Z + 12} redstone_block`)
    h.cmd(`setblock ${x} ${y} ${Z + 13} comparator[facing=north]`)
    h.cmd(`setblock ${x} ${y} ${Z + 14} redstone_wire`)
  }
  await h.tp(X + 6.5, Y + 1, Z + 15.5)
  await h.ticks(60)
  h.result('hopper_locked', `${props(X + 4, y + 1, Z + 12).enabled} chest_signal=${power(X + 4, y, Z + 14)}`)
  h.result('hopper_unlocked', `${props(X + 8, y + 1, Z + 12).enabled} chest_signal=${power(X + 8, y, Z + 14)}`)

  // Slime carries the block beside it; honey does not stick to slime.
  // Keep sticky blocks clear of the arena floor, which would exceed the push limit.
  const pistonY = y + 2
  h.cmd(`setblock ${X - 4} ${pistonY} ${Z + 14} piston[facing=east]`)
  h.cmd(`setblock ${X - 3} ${pistonY} ${Z + 14} slime_block`)
  h.cmd(`setblock ${X - 3} ${pistonY} ${Z + 15} cobblestone`)
  h.cmd(`setblock ${X - 3} ${pistonY} ${Z + 13} honey_block`)
  await h.serverTicks(SETTLE_TICKS)
  h.cmd(`setblock ${X - 5} ${pistonY} ${Z + 14} redstone_block`)
  await h.serverTicks(SETTLE_TICKS)
  requireScene(props(X - 4, pistonY, Z + 14).extended === true, 'slime piston must extend')
  h.result('slime_push', [h.blockName(X - 2, pistonY, Z + 14), h.blockName(X - 2, pistonY, Z + 15), h.blockName(X - 3, pistonY, Z + 15), h.blockName(X - 3, pistonY, Z + 13), h.blockName(X - 2, pistonY, Z + 13)].join(','))

  // A slime block pushed up launches what stands on it; a honey block pushed sideways carries it.
  h.cmd(`setblock ${X + 10} ${y + 1} ${Z - 10} piston[facing=up]`)
  h.cmd(`setblock ${X + 10} ${y + 2} ${Z - 10} slime_block`)
  h.cmd(`setblock ${X + 6} ${y + 1} ${Z - 10} piston[facing=east]`)
  h.cmd(`setblock ${X + 7} ${y + 1} ${Z - 10} honey_block`)
  h.cmd(`summon armor_stand ${X + 10.5} ${y + 3} ${Z - 9.5}`)
  h.cmd(`summon armor_stand ${X + 7.5} ${y + 2} ${Z - 9.5}`)
  await h.serverTicks(SETTLE_TICKS)
  const stand = (x) => Object.values(bot.entities).find(e => e.name === 'armor_stand' && Math.abs(e.position.x - x) < 0.2)
  const launched = stand(X + 10.5)
  const carried = stand(X + 7.5)
  requireScene(launched && carried, 'armor stands must stand on the slime and honey blocks')
  const startY = launched.position.y
  let maxY = startY
  const track = (e) => { if (e === launched) maxY = Math.max(maxY, e.position.y) }
  bot.on('entityMoved', track)
  h.cmd(`setblock ${X + 10} ${y} ${Z - 10} redstone_block`)
  h.cmd(`setblock ${X + 5} ${y + 1} ${Z - 10} redstone_block`)
  await h.serverTicks(SETTLE_TICKS)
  bot.removeListener('entityMoved', track)
  h.result('slime_launch_height', Math.round(maxY - startY))
  h.result('honey_carry_x', (carried.position.x - (X + 7.5)).toFixed(1))
})
