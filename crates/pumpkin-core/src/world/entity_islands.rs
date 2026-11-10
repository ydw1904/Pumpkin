//! Splits a tick's entities into islands that touch disjoint state. Each island ticks in list
//! order on its own thread, which gives the same result as ticking the whole list in order.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::time::Duration;

use rayon::prelude::*;

use pumpkin_data::{Block, attributes::Attributes, entity::EntityType, tag, tag::Taggable};
use pumpkin_util::math::{
    boundingbox::BoundingBox, get_section_cord, position::BlockPos, vector2::Vector2,
    vector3::Vector3,
};
use rustc_hash::FxHashMap;

use pumpkin_config::world::EntityIslandsConfig;
use pumpkin_world::chunk::CHUNK_WIDTH;

use crate::block::blocks::redstone::SIGNAL_MAX;
use crate::entity::ai::target_match::TargetMatch;
use crate::entity::ai::target_predicate::MIN_DISTANCE as MIN_TARGET_DISTANCE;
use crate::entity::living::MAX_VISIBILITY_PERCENT;
use crate::entity::player::Player;
use crate::entity::{DelayedLeash, EntityBase, PUSH_STRENGTH, tnt::TNTEntity};
use crate::plugin::api::events::entity::{
    entity_breed::EntityBreedEvent, entity_change_block::EntityChangeBlockEvent,
    entity_target::EntityTargetEvent, entity_teleport::EntityTeleportEvent,
    entity_toggle_sit::EntityToggleSitEvent, pig_zombie_anger::PigZombieAngerEvent,
};
use crate::plugin::api::events::world::generic_game::GenericGameEvent;
use crate::server::Server;
use crate::world::World;

/// Reach of same-tick neighbour updates: a full wire line, the powered block and one component.
const BLOCK_UPDATE_REACH: f64 = SIGNAL_MAX as f64 + 2.0;

/// Grid cell size for large boxes.
const CELL_SIZE: f64 = 16.0;
const SMALL_CELL_SIZE: f64 = 4.0;
/// Wider boxes, like the reach of unaudited mobs, use cells of this size.
const LARGE_CELL_SIZE: f64 = 64.0;
const MIN_COLUMN_DENSITY: usize = 32;

/// Largest speed one `Entity.push` adds.
const PUSH_SPEED: f64 = PUSH_STRENGTH * std::f64::consts::SQRT_2;

/// Extra motion covered by the contact block scan.
const SCAN_SLACK: f64 = 0.5;

/// Minimum probe margin for conservative neighbour effects.
const PUSH_SLACK: f64 = 3.0 * PUSH_SPEED;

// Larger regions retain the conservative contact-write fallback.
const MAX_CONTACT_SECTIONS: i64 = 64;
const MAX_CONTACT_BLOCKS: usize = 4096;

// Cell operations and candidate visits allowed in each planning phase.
const MAX_PLANNING_WORK: usize = 262_144;

// Cells used to group entities before exact partitioning. Larger combined bounds leave
// everything in one group.
const GROUP_CELL_SIZE: f64 = 64.0;
const MAX_GROUP_CELLS: usize = 1024;
/// Pushers an audited entity may count while its group plans on its own.
const MAX_GROUPED_PUSHERS: usize = 256;

struct PlanningBudget {
    remaining: usize,
}

impl Default for PlanningBudget {
    fn default() -> Self {
        Self {
            remaining: MAX_PLANNING_WORK,
        }
    }
}

impl PlanningBudget {
    fn spend(&mut self, work: usize) -> Option<()> {
        self.remaining = self.remaining.checked_sub(work)?;
        Some(())
    }

    fn cells(
        &mut self,
        area: &BoundingBox,
        size: f64,
    ) -> Option<[std::ops::RangeInclusive<i32>; 2]> {
        // Block reads intentionally span all Y levels.
        if area.min.y.is_nan() || area.max.y.is_nan() || area.min.y > area.max.y {
            return None;
        }
        let x = cells_of(area.min.x, area.max.x, size)?;
        let z = cells_of(area.min.z, area.max.z, size)?;
        self.spend(cell_count(&x)?.checked_mul(cell_count(&z)?)?)?;
        Some([x, z])
    }
}

pub(super) const MIN_PARALLEL_ENTITIES: usize = 16;
/// Entities with a global reach split the tick into runs planned one after another. Past this
/// many runs the rest of the tick runs in list order.
pub(super) const MAX_PLANNED_RUNS: usize = 4;
const SERIAL_PROBE_INTERVAL: u8 = 20;
const PARALLEL_SHARE_DIVISOR: usize = 4;
/// A serial preference also ends once the entity count moves this share away from the count
/// it was planned with. Spawns and despawns change the count almost every tick in a busy area.
const RECOUNT_SHARE_DIVISOR: usize = 8;
/// A new tick time makes up one part in this many of the averaged serial or planned tick cost.
const COST_SMOOTHING: u32 = 4;

/// A dominant island leaves too little parallel work to pay for planning.
pub(super) const fn worth_parallel(largest_island: usize, entity_count: usize) -> bool {
    largest_island < entity_count - entity_count / PARALLEL_SHARE_DIVISOR
}

/// Pool threads per entity a parallel job ticks at least. Each job tends to land on another idle
/// core, which on a large pool wakes from deep sleep with cold caches, costing more than a
/// small island's tick.
const THREADS_PER_JOB_ENTITY: usize = 10;

/// Splits largest-first islands into consecutive ranges, each ticked by one job, holding at
/// least one entity per [`THREADS_PER_JOB_ENTITY`] pool threads; the last range may hold fewer.
pub(super) fn island_jobs(islands: &[Vec<usize>], threads: usize) -> Vec<std::ops::Range<usize>> {
    let min_entities = threads / THREADS_PER_JOB_ENTITY;
    let mut jobs = Vec::new();
    let mut first = 0;
    let mut entities = 0;
    for (index, island) in islands.iter().enumerate() {
        entities += island.len();
        if entities >= min_entities {
            jobs.push(first..index + 1);
            first = index + 1;
            entities = 0;
        }
    }
    if first < islands.len() {
        jobs.push(first..islands.len());
    }
    jobs
}

#[derive(Default)]
pub(super) struct Schedule {
    entity_count: usize,
    planned_count: usize,
    ticks_until_probe: u8,
    /// Parallel ticks since the serial cost was last measured.
    parallel_ticks: u8,
    /// Entity tick time per entity of recent serial and planned ticks.
    serial_cost: Option<Duration>,
    planned_cost: Option<Duration>,
}

impl Schedule {
    pub(super) const fn should_plan(&mut self, entity_count: usize) -> bool {
        self.entity_count = entity_count;
        if entity_count < MIN_PARALLEL_ENTITIES {
            self.ticks_until_probe = 0;
            return false;
        }
        if self.ticks_until_probe > 0
            && entity_count.abs_diff(self.planned_count)
                <= self.planned_count / RECOUNT_SHARE_DIVISOR
        {
            self.ticks_until_probe -= 1;
            return false;
        }
        true
    }

    /// Records a tick that planned islands and took `elapsed` for its entities, and returns
    /// whether the next ticks should plan too.
    ///
    /// Planning and waking pool threads can cost more than the parallel ticking saves, as with
    /// few or tiny islands, or on a large pool of idle cores. So a planned tick also loses to a
    /// serial tick that was cheaper per entity, measured at least every
    /// [`SERIAL_PROBE_INTERVAL`] parallel ticks. Both ways tick the same, so the choice only
    /// changes how long the tick takes.
    pub(super) fn record_plan(&mut self, largest_island: usize, elapsed: Duration) -> bool {
        self.planned_count = self.entity_count;
        let cost = average(self.planned_cost, per_entity(elapsed, self.entity_count));
        self.planned_cost = Some(cost);
        let parallel = worth_parallel(largest_island, self.entity_count)
            && self.serial_cost.is_none_or(|serial| cost <= serial);
        // Only the serial preference is cached; parallel ticks always use a fresh plan, apart
        // from a serial tick now and then to keep the comparison current.
        self.ticks_until_probe = if !parallel {
            SERIAL_PROBE_INTERVAL - 1
        } else if self.serial_cost.is_none() || self.parallel_ticks + 1 >= SERIAL_PROBE_INTERVAL {
            self.parallel_ticks = 0;
            1
        } else {
            self.parallel_ticks += 1;
            0
        };
        parallel
    }

    /// Records a tick of at least [`MIN_PARALLEL_ENTITIES`] entities that ran in list order
    /// without planning and took `elapsed` for its entities.
    pub(super) fn record_serial(&mut self, elapsed: Duration) {
        if self.entity_count < MIN_PARALLEL_ENTITIES {
            return;
        }
        self.serial_cost = Some(average(
            self.serial_cost,
            per_entity(elapsed, self.entity_count),
        ));
    }
}

fn per_entity(elapsed: Duration, entity_count: usize) -> Duration {
    elapsed / u32::try_from(entity_count.max(1)).unwrap_or(u32::MAX)
}

/// Keeps most of the earlier costs, so one slow tick, like one that finds a path or waits on a
/// descheduled worker, doesn't decide on its own.
fn average(previous: Option<Duration>, cost: Duration) -> Duration {
    previous.map_or(cost, |previous| {
        (previous * (COST_SMOOTHING - 1) + cost) / COST_SMOOTHING
    })
}

/// A plugin listens to an event an entity tick can fire.
///
/// Targeting, breeding, anger, a sit order dropped on damage, a teleport, a block an enderman
/// takes or a game event. Its handler may touch anything, so the tick runs in list order.
#[must_use]
pub fn plugins_observe_entity_ticks(server: &Server) -> bool {
    let plugins = &server.plugin_manager;
    plugins.has_handlers::<EntityTargetEvent>()
        || plugins.has_handlers::<EntityBreedEvent>()
        || plugins.has_handlers::<PigZombieAngerEvent>()
        || plugins.has_handlers::<EntityToggleSitEvent>()
        || plugins.has_handlers::<EntityTeleportEvent>()
        || plugins.has_handlers::<EntityChangeBlockEvent>()
        || plugins.has_handlers::<GenericGameEvent>()
}

/// How far target searches can find players this tick. Planning refreshes it before the reports.
pub struct PlayerSearch {
    /// Highest visibility factor a player's equipment can give, as `f64` bits.
    visibility: AtomicU64,
    /// Some player is neither in creative nor in spectator mode.
    attackable: AtomicBool,
}

impl Default for PlayerSearch {
    fn default() -> Self {
        Self {
            visibility: AtomicU64::new(MAX_VISIBILITY_PERCENT.to_bits()),
            attackable: AtomicBool::new(true),
        }
    }
}

impl PlayerSearch {
    pub(crate) fn refresh(&self, players: &[Arc<Player>]) {
        let visible = players.iter().filter(|player| !player.is_spectator());
        let visibility = visible
            .clone()
            .map(|player| player.living_entity.max_visibility_percent())
            .fold(0.0, f64::max);
        self.visibility.store(visibility.to_bits(), Relaxed);
        self.attackable.store(
            visible.into_iter().any(|player| !player.is_creative()),
            Relaxed,
        );
    }

    /// Farthest a player passes a visibility-scaled range test of `range`.
    #[must_use]
    pub fn reach(&self, range: f64) -> f64 {
        let visibility = f64::from_bits(self.visibility.load(Relaxed));
        if visibility > 0.0 {
            (range * visibility).max(MIN_TARGET_DISTANCE)
        } else {
            0.0
        }
    }

    /// [`Self::reach`] for a combat search, which skips creative players.
    #[must_use]
    pub fn attackable_reach(&self, range: f64) -> f64 {
        if self.attackable.load(Relaxed) {
            self.reach(range)
        } else {
            0.0
        }
    }
}

/// What one entity can read or change next tick.
#[derive(Clone, Debug)]
pub struct TickReach {
    /// Space it can occupy during the tick.
    pub body: BoundingBox,
    /// Touches entities whose body overlaps this.
    pub entities: BoundingBox,
    /// Touches players whose body overlaps this.
    pub players: BoundingBox,
    /// Touches entities of the matched types whose body overlaps the box.
    pub typed_entities: Vec<(BoundingBox, TargetMatch)>,
    /// The entity's type, for other entities' typed searches. `None` matches every search,
    /// for entities that can spawn or turn into other types during the tick.
    pub kind: Option<&'static EntityType>,
    pub block_reads: BoundingBox,
    pub block_writes: Option<BoundingBox>,
    /// Can act anywhere in the world.
    pub global: bool,
    /// Entities and players touched at any distance.
    pub links: Vec<i32>,
    /// Links whose eyes it checks the line of sight to: block reads also span their bodies.
    pub sight: Vec<i32>,
    /// Can target an attacker found during the tick: block reads also span its contacts.
    pub spans_contacts: bool,
}

impl TickReach {
    /// Anything within `radius` of the box.
    #[must_use]
    pub fn conservative(bounding_box: &BoundingBox, radius: f64, links: Vec<i32>) -> Self {
        let body = bounding_box.expand(radius, radius, radius);
        let block_writes = body.expand(BLOCK_UPDATE_REACH, BLOCK_UPDATE_REACH, BLOCK_UPDATE_REACH);
        Self {
            body,
            entities: body,
            players: body,
            typed_entities: Vec::new(),
            kind: None,
            block_reads: block_writes,
            block_writes: Some(block_writes),
            global: false,
            links,
            sight: Vec::new(),
            spans_contacts: false,
        }
    }
}

/// Reach of an audited entity, as distances from its body.
#[derive(Clone, Debug)]
pub struct AuditedReach {
    pub bounding_box: BoundingBox,
    /// Horizontal and vertical distance it can move by itself.
    pub motion: (f64, f64),
    pub entities: f64,
    pub players: f64,
    /// Searches that only touch entities of the matched types.
    pub typed_entities: Vec<(f64, TargetMatch)>,
    /// Its type; `None` while it can spawn other types, which any typed search may match.
    pub kind: Option<&'static EntityType>,
    pub block_reads: f64,
    /// Area where block changes make it recompute its path.
    pub route: Option<BoundingBox>,
    pub changes_blocks: bool,
    pub global: bool,
    pub links: Vec<i32>,
    /// Links whose eyes it checks the line of sight to.
    pub sight: Vec<i32>,
    /// Can target an attacker found during the tick and check the sight to it.
    pub spans_contacts: bool,
    /// Horizontal distance from its body at which it can knock entities back, 0 for none.
    pub knockback: f64,
}

impl AuditedReach {
    /// A reach that can act anywhere, so the tick runs alone.
    #[must_use]
    pub const fn global(bounding_box: BoundingBox) -> Self {
        Self {
            bounding_box,
            motion: (0.0, 0.0),
            entities: 0.0,
            players: 0.0,
            typed_entities: Vec::new(),
            kind: None,
            block_reads: 0.0,
            route: None,
            changes_blocks: true,
            global: true,
            links: Vec::new(),
            sight: Vec::new(),
            spans_contacts: false,
            knockback: 0.0,
        }
    }

    /// Covers a body teleported anywhere into `area`, which then moves and acts on from there.
    pub fn add_teleport(&mut self, world: &World, area: &BoundingBox) {
        let (horizontal, vertical) = self.motion;
        let reached = area
            .expand(
                horizontal + SCAN_SLACK,
                vertical + SCAN_SLACK,
                horizontal + SCAN_SLACK,
            )
            .expand_towards(0.0, -1.0, 0.0);
        self.scan_teleport_contacts(world, &reached);
        self.bounding_box = union(&self.bounding_box, area);
    }

    /// Flags what a body in `area` can touch: block contacts there change blocks if any loaded
    /// section of it holds such a block, and an end gateway can send it anywhere.
    fn scan_teleport_contacts(&mut self, world: &World, area: &BoundingBox) {
        let (min, max) = (area.min_block_pos().0, area.max_block_pos().0);
        let bottom = get_section_cord(world.get_bottom_y());
        let top = get_section_cord(world.get_top_y());
        for x in get_section_cord(min.x)..=get_section_cord(max.x) {
            for z in get_section_cord(min.z)..=get_section_cord(max.z) {
                // Teleports never land in unloaded chunks.
                if world
                    .level
                    .read_chunk_sync(&Vector2::new(x, z), |_| ())
                    .is_none()
                {
                    continue;
                }
                for y in get_section_cord(min.y).max(bottom)..=get_section_cord(max.y).min(top) {
                    if !contact_section_may_change_blocks(world, x, y, z) {
                        continue;
                    }
                    self.changes_blocks = true;
                    if section_holds(world, x, y, z, |block| block.id == Block::END_GATEWAY.id) {
                        self.global = true;
                        return;
                    }
                }
            }
        }
    }

    /// Flags blocks it can touch that change or teleport on contact.
    pub fn scan_contact_blocks(&mut self, world: &World) {
        let area = self
            .bounding_box
            .expand(
                self.motion.0 + SCAN_SLACK,
                self.motion.1 + SCAN_SLACK,
                self.motion.0 + SCAN_SLACK,
            )
            // Blocks stepped and landed on.
            .expand_towards(0.0, -1.0, 0.0);
        let blocks = (|| {
            let x = cells_of(area.min.x, area.max.x, 1.0)?;
            let y = cells_of(area.min.y, area.max.y, 1.0)?;
            let z = cells_of(area.min.z, area.max.z, 1.0)?;
            cell_count(&x)?
                .checked_mul(cell_count(&y)?)?
                .checked_mul(cell_count(&z)?)
        })();
        if blocks.is_none_or(|blocks| blocks > MAX_CONTACT_BLOCKS) {
            self.global = true;
            return;
        }
        let (min, max) = (area.min_block_pos().0, area.max_block_pos().0);
        for x in get_section_cord(min.x)..=get_section_cord(max.x) {
            for z in get_section_cord(min.z)..=get_section_cord(max.z) {
                for y in get_section_cord(min.y.max(world.get_bottom_y()))
                    ..=get_section_cord(max.y.min(world.get_top_y()))
                {
                    if !contact_section_may_change_blocks(world, x, y, z) {
                        continue;
                    }
                    let start = Vector3::new(x, y, z) * CHUNK_WIDTH as i32;
                    let end = start.add_raw(
                        CHUNK_WIDTH as i32 - 1,
                        CHUNK_WIDTH as i32 - 1,
                        CHUNK_WIDTH as i32 - 1,
                    );
                    // `World::get_block` for each block of the section, reading the chunk once.
                    // An unloaded chunk reads as air.
                    let (global, changes) = world
                        .level
                        .read_chunk_sync(&Vector2::new(x, z), |chunk| {
                            let (mut global, mut changes) = (false, false);
                            for x in min.x.max(start.x)..=max.x.min(end.x) {
                                for y in min.y.max(start.y)..=max.y.min(end.y) {
                                    for z in min.z.max(start.z)..=max.z.min(end.z) {
                                        let pos = BlockPos::new(x, y, z);
                                        let (_, relative) = pos.chunk_and_chunk_relative_position();
                                        let block = world
                                            .is_in_build_limit(pos)
                                            .then(|| {
                                                chunk.section.get_block_absolute_y(
                                                    relative.x as usize,
                                                    relative.y,
                                                    relative.z as usize,
                                                )
                                            })
                                            .flatten()
                                            .map_or(&Block::AIR, Block::from_state_id);
                                        if block.id == Block::END_GATEWAY.id {
                                            global = true;
                                        } else if changes_on_contact(block) {
                                            changes = true;
                                        }
                                    }
                                }
                            }
                            (global, changes)
                        })
                        .unwrap_or_default();
                    self.global |= global;
                    self.changes_blocks |= changes;
                }
            }
        }
    }
}

/// Blocks whose contact, step or landing callbacks change blocks or change dimension.
fn changes_on_contact(block: &Block) -> bool {
    block.has_tag(&tag::Block::MINECRAFT_PRESSURE_PLATES)
        || [
            &Block::DETECTOR_RAIL,
            &Block::TRIPWIRE,
            &Block::REDSTONE_ORE,
            &Block::DEEPSLATE_REDSTONE_ORE,
            &Block::BIG_DRIPLEAF,
            &Block::TURTLE_EGG,
            &Block::POWDER_SNOW,
            &Block::SCULK_SHRIEKER,
            &Block::SCULK_SENSOR,
            &Block::CALIBRATED_SCULK_SENSOR,
            &Block::LILY_PAD,
            &Block::FARMLAND,
            &Block::NETHER_PORTAL,
            &Block::END_PORTAL,
        ]
        .iter()
        .any(|candidate| candidate.id == block.id)
}

#[derive(Clone)]
pub enum EntityReach {
    Audited(AuditedReach),
    Conservative(TickReach),
}

#[derive(Clone, Copy)]
pub(crate) enum BlockWriteCheck {
    Unconditional,
    Contact,
}

impl EntityReach {
    pub(super) const fn is_global(&self) -> bool {
        match self {
            Self::Audited(reach) => reach.global,
            Self::Conservative(reach) => reach.global,
        }
    }

    fn links(&self) -> &[i32] {
        match self {
            Self::Audited(reach) => &reach.links,
            Self::Conservative(reach) => &reach.links,
        }
    }

    #[must_use]
    pub(crate) const fn block_write_check(&self) -> BlockWriteCheck {
        match self {
            Self::Audited(audited) if !audited.changes_blocks => BlockWriteCheck::Contact,
            _ => BlockWriteCheck::Unconditional,
        }
    }

    #[must_use]
    pub fn of(entity: &dyn EntityBase, config: &EntityIslandsConfig) -> Self {
        if let Some(reach) = entity.audited_reach() {
            return Self::Audited(reach);
        }
        let base = entity.get_entity();
        let movement = base.velocity.load().length()
            + entity
                .get_mob()
                .map_or(0.0, crate::entity::mob::Mob::additional_tick_movement);
        let radius = movement
            + config.unaudited_reach
            + entity.get_living_entity().map_or(0.0, |living| {
                living
                    .get_attribute_value(&Attributes::FOLLOW_RANGE)
                    .max(0.0)
            });
        let mut reach =
            TickReach::conservative(&base.bounding_box.load(), radius, links_of(entity));
        if let Some(mob) = entity.get_mob() {
            let player_radius = radius.max(mob.player_tick_range() + movement);
            reach.players =
                base.bounding_box
                    .load()
                    .expand(player_radius, player_radius, player_radius);
        }
        // Blast power and chained block effects can exceed the configured margins.
        reach.global = entity
            .get_mob()
            .is_some_and(crate::entity::mob::Mob::has_global_tick_reach)
            || entity
                .cast_any()
                .downcast_ref::<TNTEntity>()
                .is_some_and(TNTEntity::is_about_to_explode);
        Self::Conservative(reach)
    }
}

/// Vehicle, passengers, leash holder, owner, target and recent attackers.
#[must_use]
pub fn links_of(entity: &dyn EntityBase) -> Vec<i32> {
    let base = entity.get_entity();
    let mut links: Vec<i32> = base
        .passengers
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .map(|passenger| passenger.get_entity().entity_id)
        .collect();
    for other in [&base.vehicle, &base.leashed_to] {
        if let Some(other) = other
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            links.push(other.get_entity().entity_id);
        }
    }
    if let Some(DelayedLeash::Entity(uuid)) = *base
        .delayed_leash
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
    {
        let world = base.world.load();
        links.extend(
            world
                .get_entity_by_uuid(uuid)
                .map(|holder| holder.get_entity().entity_id)
                .or_else(|| {
                    world
                        .get_player_by_uuid(uuid)
                        .map(|player| player.entity_id())
                }),
        );
    }
    if let Some(owner) = entity.get_owner_entity() {
        links.push(owner.get_entity().entity_id);
    }
    if let Some(fish) = entity.get_mob().and_then(|mob| mob.as_schooling_fish())
        && let Some(leader) = fish.leader()
    {
        links.push(leader.get_entity().entity_id);
    }
    if let Some(mob) = entity.get_mob() {
        links.extend(mob.tick_links());
        // `NeutralMob::restore_persistent_target` looks the saved grudge up by UUID.
        if let Some(neutral) = mob.as_neutral()
            && neutral.get_neutral_data().restore_target.load(Relaxed)
            && let Some(uuid) = neutral.get_persistent_anger_target()
        {
            links.extend(
                crate::entity::mob::neutral::find_by_uuid(&base.world.load(), uuid)
                    .map(|target| target.get_entity().entity_id),
            );
        }
        if let Some(target) = mob.get_mob_entity().get_target() {
            links.push(target.get_entity().entity_id);
        }
    }
    if let Some(living) = entity.get_living_entity() {
        // Kill credit on death.
        links.extend(
            living
                .get_last_hurt_by_player()
                .map(|player| player.entity_id()),
        );
        links.extend(living.get_last_hurt_by_mob_id().filter(|id| *id != 0));
    }
    links
}

/// An entity that doesn't tick this tick: id, body and type.
pub type IdleEntity = (i32, BoundingBox, &'static EntityType);

/// What damage dealt in another entity's turn can make an entity touch beyond its body, like a
/// teleport away from the hit.
#[derive(Clone, Copy, Debug)]
pub struct HurtReach {
    pub area: BoundingBox,
    pub changes_blocks: bool,
}

impl HurtReach {
    /// A node that stands in for these reactions of an entity with `body` and `kind`. It never
    /// ticks, but joins whatever the reactions can touch with whatever hits the entity.
    #[must_use]
    pub fn stand_in(
        &self,
        body: BoundingBox,
        kind: &'static EntityType,
        links: Vec<i32>,
    ) -> TickReach {
        TickReach {
            body,
            entities: self.area,
            players: self.area,
            typed_entities: Vec::new(),
            kind: Some(kind),
            block_reads: self.area,
            block_writes: self.changes_blocks.then(|| {
                self.area
                    .expand(BLOCK_UPDATE_REACH, BLOCK_UPDATE_REACH, BLOCK_UPDATE_REACH)
            }),
            global: false,
            links,
            sight: Vec::new(),
            spans_contacts: false,
        }
    }
}

/// Turns reports into reaches.
///
/// Audited bodies grow by the pushes they can receive, and by
/// `knockback_reach` within reach of an unaudited entity or the hits of an earlier audited one.
/// Returns `None` when the tick needs serial execution or planning exceeds its budget.
#[must_use]
pub fn resolve(reports: Vec<EntityReach>, config: &EntityIslandsConfig) -> Option<Vec<TickReach>> {
    resolve_bounded(reports, config, usize::MAX)
}

/// [`resolve`], but also `None` when an audited entity counts more than `max_pushers`.
fn resolve_bounded(
    reports: Vec<EntityReach>,
    config: &EntityIslandsConfig,
    max_pushers: usize,
) -> Option<Vec<TickReach>> {
    if reports.iter().any(EntityReach::is_global) {
        return None;
    }
    let mut budget = PlanningBudget::default();
    budget.spend(reports.len())?;
    let mut pushers = Grid::default();
    let mut threats = Grid::default();
    for report in &reports {
        if let EntityReach::Conservative(reach) = report {
            pushers.insert(pushers.boxes.len(), reach.body, &mut budget)?;
            threats.insert(threats.boxes.len(), reach.entities, &mut budget)?;
        }
    }
    let mut query = Query::new(reports.len());
    let mut reaches = Vec::with_capacity(reports.len());
    for report in reports {
        match report {
            EntityReach::Conservative(reach) => reaches.push(reach),
            EntityReach::Audited(audited) => {
                let mut count = 0;
                pushers.for_each_overlapping(
                    &mut query,
                    &audited.bounding_box,
                    &mut budget,
                    |_| count += 1,
                )?;
                if count > max_pushers {
                    return None;
                }
                let pushes = PUSH_SPEED * count as f64;
                let probe = grown_body(&audited, pushes + config.knockback_reach.max(PUSH_SLACK));
                let mut threatened = false;
                threats
                    .for_each_overlapping(&mut query, &probe, &mut budget, |_| threatened = true)?;
                let extra = pushes
                    + if threatened {
                        config.knockback_reach
                    } else {
                        0.0
                    };
                let knockback = audited.knockback;
                let reach = finish(audited, extra);
                // Like its pushes, its hits only throw entities that move after it.
                if knockback > 0.0 {
                    threats.insert(
                        threats.boxes.len(),
                        reach.body.expand(knockback, 0.0, knockback),
                        &mut budget,
                    )?;
                }
                // Travel precedes pushing. Later audited turns can only change this body's
                // velocity for the next tick, so they do not grow its current movement bound.
                pushers.insert(pushers.boxes.len(), reach.body, &mut budget)?;
                reaches.push(reach);
            }
        }
    }
    Some(reaches)
}

/// Removes contact-write fallbacks that no writer can make necessary.
pub(crate) fn prune_block_writes(
    world: &World,
    reaches: &mut [TickReach],
    checks: &[BlockWriteCheck],
) -> Option<()> {
    if reaches.iter().any(|reach| reach.global) {
        return Some(());
    }
    let mut budget = PlanningBudget::default();
    budget.spend(reaches.len())?;
    let mut candidates = Vec::new();
    let mut contacts = Grid::default();
    let mut sections = FxHashMap::default();
    let mut active: Vec<bool> = reaches
        .iter()
        .map(|reach| reach.block_writes.is_some())
        .collect();
    for (slot, reach) in reaches.iter().enumerate() {
        if !active[slot] {
            continue;
        }
        if matches!(checks[slot], BlockWriteCheck::Unconditional) {
            continue;
        }
        let area = reach.body.expand_towards(0.0, -1.0, 0.0);
        let contact = BoundingBox::new(
            area.min_block_pos().0.to_f64(),
            area.max_block_pos().0.to_f64() + Vector3::new(1.0, 1.0, 1.0),
        );
        if contact_may_change_blocks(world, &contact, &mut sections) {
            continue;
        }
        active[slot] = false;
        contacts.insert(candidates.len(), contact, &mut budget)?;
        candidates.push(slot);
    }
    if candidates.is_empty() {
        return Some(());
    }
    let mut pending: VecDeque<_> = active
        .iter()
        .enumerate()
        .filter_map(|(slot, active)| active.then_some(slot))
        .collect();
    let mut query = Query::new(candidates.len());
    while let Some(slot) = pending.pop_front() {
        let Some(writes) = reaches[slot].block_writes else {
            continue;
        };
        contacts.for_each_overlapping(&mut query, &writes, &mut budget, |candidate| {
            let slot = candidates[candidate];
            if !active[slot] {
                active[slot] = true;
                pending.push_back(slot);
            }
        })?;
    }
    for slot in candidates {
        if !active[slot] {
            reaches[slot].block_writes = None;
        }
    }
    Some(())
}

fn contact_may_change_blocks(
    world: &World,
    contact: &BoundingBox,
    sections: &mut FxHashMap<(i32, i32, i32), bool>,
) -> bool {
    if [
        contact.min.x,
        contact.min.y,
        contact.min.z,
        contact.max.x,
        contact.max.y,
        contact.max.z,
    ]
    .iter()
    .any(|value| !value.is_finite())
    {
        return true;
    }
    let bottom = world.dimension.min_y;
    let top = bottom + world.dimension.height;
    let min_y = (contact.min.y as i32).max(bottom);
    let max_y = ((contact.max.y - 1.0) as i32).min(top - 1);
    if min_y > max_y {
        return false;
    }
    let min = [
        get_section_cord(contact.min.x as i32),
        get_section_cord(min_y),
        get_section_cord(contact.min.z as i32),
    ];
    let max = [
        get_section_cord((contact.max.x - 1.0) as i32),
        get_section_cord(max_y),
        get_section_cord((contact.max.z - 1.0) as i32),
    ];
    let volume = (0..3).fold(1i64, |volume, axis| {
        volume.saturating_mul(i64::from(max[axis]) - i64::from(min[axis]) + 1)
    });
    if volume > MAX_CONTACT_SECTIONS {
        return true;
    }
    for x in min[0]..=max[0] {
        for z in min[2]..=max[2] {
            for y in min[1]..=max[1] {
                if *sections
                    .entry((x, y, z))
                    .or_insert_with(|| contact_section_may_change_blocks(world, x, y, z))
                {
                    return true;
                }
            }
        }
    }
    false
}

fn contact_section_may_change_blocks(world: &World, x: i32, y: i32, z: i32) -> bool {
    // Island planning checks each section once for all reports.
    if let Some(sections) = world.contact_sections.load().as_ref() {
        if let Some(changes) = sections.get(&(x, y, z)) {
            return *changes;
        }
        let changes = scan_contact_section(world, x, y, z);
        sections.insert((x, y, z), changes);
        return changes;
    }
    scan_contact_section(world, x, y, z)
}

fn scan_contact_section(world: &World, x: i32, y: i32, z: i32) -> bool {
    section_holds(world, x, y, z, |block| {
        block.id == Block::END_GATEWAY.id || changes_on_contact(block)
    })
}

/// Whether the section's palette holds a matching block. Unloaded sections may hold anything.
fn section_holds(world: &World, x: i32, y: i32, z: i32, matches: impl Fn(&Block) -> bool) -> bool {
    world
        .level
        .read_chunk_sync(&Vector2::new(x, z), |chunk| {
            let blocks = chunk
                .section
                .block_sections
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let index = y - get_section_cord(chunk.section.min_y);
            usize::try_from(index)
                .ok()
                .and_then(|index| blocks.get(index))
                .is_none_or(|palette| {
                    palette.contains_matching(|state| matches(Block::from_state_id(state)))
                })
        })
        .unwrap_or(true)
}

fn grown_body(audited: &AuditedReach, extra: f64) -> BoundingBox {
    let (horizontal, vertical) = audited.motion;
    audited
        .bounding_box
        .expand(horizontal + extra, vertical + extra, horizontal + extra)
}

/// The boxes [`finish`] gives an audited reach.
struct FinishedAreas {
    body: BoundingBox,
    entities: BoundingBox,
    players: BoundingBox,
    block_reads: BoundingBox,
    block_writes: Option<BoundingBox>,
}

impl FinishedAreas {
    fn of(audited: &AuditedReach, extra: f64) -> Self {
        let body = grown_body(audited, extra);
        let around = |distance: f64| body.expand(distance, distance, distance);
        // Player pickup range in `World::tick`.
        let players = body.expand(
            audited.players.max(1.0),
            audited.players.max(0.5),
            audited.players.max(1.0),
        );
        let changes_blocks = audited.changes_blocks || extra > SCAN_SLACK;
        let block_writes = changes_blocks.then(|| around(BLOCK_UPDATE_REACH));
        let mut block_reads = around(if changes_blocks {
            audited.block_reads.max(BLOCK_UPDATE_REACH)
        } else {
            audited.block_reads
        });
        if let Some(route) = audited.route {
            block_reads = union(&block_reads, &route);
        }
        // Searches follow water columns down to the bottom of the world.
        block_reads.min.y = f64::NEG_INFINITY;
        block_reads.max.y = f64::INFINITY;
        Self {
            body,
            entities: around(audited.entities),
            players,
            block_reads,
            block_writes,
        }
    }

    fn around(&self, distance: f64) -> BoundingBox {
        self.body.expand(distance, distance, distance)
    }
}

fn finish(audited: AuditedReach, extra: f64) -> TickReach {
    let areas = FinishedAreas::of(&audited, extra);
    TickReach {
        body: areas.body,
        entities: areas.entities,
        players: areas.players,
        typed_entities: audited
            .typed_entities
            .into_iter()
            .map(|(distance, target)| (areas.around(distance), target))
            .collect(),
        kind: audited.kind,
        block_reads: areas.block_reads,
        block_writes: areas.block_writes,
        global: audited.global,
        links: audited.links,
        sight: audited.sight,
        spans_contacts: audited.spans_contacts,
    }
}

/// [`total_bounds`] of the [`finish`]ed reach, without building it.
fn finished_bounds(audited: &AuditedReach, extra: f64) -> BoundingBox {
    let areas = FinishedAreas::of(audited, extra);
    let mut bounds = union(&union(&areas.body, &areas.entities), &areas.players);
    for (distance, _) in &audited.typed_entities {
        bounds = union(&bounds, &areas.around(*distance));
    }
    bounds = union(&bounds, &areas.block_reads);
    if let Some(writes) = areas.block_writes {
        bounds = union(&bounds, &writes);
    }
    bounds
}

pub(crate) const fn union(a: &BoundingBox, b: &BoundingBox) -> BoundingBox {
    BoundingBox::new(
        pumpkin_util::math::vector3::Vector3::new(
            a.min.x.min(b.min.x),
            a.min.y.min(b.min.y),
            a.min.z.min(b.min.z),
        ),
        pumpkin_util::math::vector3::Vector3::new(
            a.max.x.max(b.max.x),
            a.max.y.max(b.max.y),
            a.max.z.max(b.max.z),
        ),
    )
}

pub struct Islands {
    /// Node indices in list order.
    pub islands: Vec<Vec<usize>>,
    #[cfg(debug_assertions)]
    pub island_of: Arc<IslandMap>,
}

/// Island of each entity and player.
pub type IslandMap = FxHashMap<i32, usize>;

/// Plans this tick's islands from each ticking entity's ID, report and block write check.
/// `stand_ins` are nodes after the ticking ones that join the [`HurtReach`] of entities with
/// what hits them; they leave no slot in the islands.
/// Groups that can't touch each other resolve, prune and partition on their own and in
/// parallel. Returns `None` when the tick needs serial execution or planning exceeds its budget.
#[expect(
    clippy::too_many_arguments,
    reason = "Everything one tick's plan depends on, gathered by `World::tick`."
)]
pub(crate) fn plan(
    world: &World,
    ids: &[i32],
    reports: Vec<EntityReach>,
    checks: &[BlockWriteCheck],
    stand_ins: &[(i32, TickReach)],
    idle_entities: &[IdleEntity],
    players: &[(i32, BoundingBox)],
    config: &EntityIslandsConfig,
) -> Option<Islands> {
    if reports.iter().any(EntityReach::is_global) {
        return None;
    }
    let ticking = ids.len();
    let mut islands = plan_with_stand_ins(
        world,
        ids,
        reports,
        checks,
        stand_ins,
        idle_entities,
        players,
        config,
    )?;
    for island in &mut islands.islands {
        island.retain(|&slot| slot < ticking);
    }
    Some(islands)
}

#[expect(
    clippy::too_many_arguments,
    reason = "Everything one tick's plan depends on, passed through from `plan`."
)]
fn plan_with_stand_ins(
    world: &World,
    ids: &[i32],
    reports: Vec<EntityReach>,
    checks: &[BlockWriteCheck],
    stand_ins: &[(i32, TickReach)],
    idle_entities: &[IdleEntity],
    players: &[(i32, BoundingBox)],
    config: &EntityIslandsConfig,
) -> Option<Islands> {
    let ticking = ids.len();
    // Audited bodies grow with the pushers they count, so their group bounds assume the most a
    // group lets them count.
    let grouped_extra =
        PUSH_SPEED * MAX_GROUPED_PUSHERS as f64 + config.knockback_reach.max(PUSH_SLACK);
    let bounds: Vec<_> = ids
        .iter()
        .zip(&reports)
        .map(|(id, report)| {
            let bounds = match report {
                EntityReach::Conservative(reach) => total_bounds(reach),
                EntityReach::Audited(audited) => finished_bounds(audited, grouped_extra),
            };
            (*id, bounds)
        })
        .chain(
            stand_ins
                .iter()
                .map(|(id, reach)| (*id, total_bounds(reach))),
        )
        .collect();
    let groups = groups(
        &bounds,
        |slot| {
            reports.get(slot).map_or_else(
                || stand_ins[slot - ticking].1.links.as_slice(),
                EntityReach::links,
            )
        },
        idle_entities,
        players,
    );
    let grouped = groups
        .par_iter()
        .map(|group| {
            // Stand-ins follow the ticking nodes in the group, like in the whole tick.
            let group_reports = group
                .nodes
                .iter()
                .filter(|&&slot| slot < ticking)
                .map(|&slot| reports[slot].clone())
                .collect();
            let mut reaches = resolve_bounded(group_reports, config, MAX_GROUPED_PUSHERS)?;
            reaches.extend(
                group
                    .nodes
                    .iter()
                    .filter(|&&slot| slot >= ticking)
                    .map(|&slot| stand_ins[slot - ticking].1.clone()),
            );
            let group_checks: Vec<_> = group
                .nodes
                .iter()
                .map(|&slot| {
                    checks
                        .get(slot)
                        .copied()
                        .unwrap_or(BlockWriteCheck::Unconditional)
                })
                .collect();
            prune_block_writes(world, &mut reaches, &group_checks)?;
            let nodes: Vec<_> = group
                .nodes
                .iter()
                .map(|&slot| {
                    ids.get(slot)
                        .copied()
                        .unwrap_or_else(|| stand_ins[slot - ticking].0)
                })
                .zip(reaches)
                .collect();
            let local = Group {
                nodes: (0..nodes.len()).collect(),
                idle_entities: group.idle_entities.clone(),
                players: group.players.clone(),
            };
            let mut islands = partition_group(&nodes, idle_entities, players, &local)?;
            for slot in islands.islands.iter_mut().flatten() {
                *slot = group.nodes[*slot];
            }
            Some(islands)
        })
        .collect::<Option<Vec<_>>>();
    if let Some(parts) = grouped {
        return Some(merge(parts));
    }
    // A group exceeded its pusher bound or budget, so plan the whole tick at once.
    let mut reaches = resolve(reports, config)?;
    reaches.extend(stand_ins.iter().map(|(_, reach)| reach.clone()));
    let mut checks = checks.to_vec();
    checks.resize(reaches.len(), BlockWriteCheck::Unconditional);
    prune_block_writes(world, &mut reaches, &checks)?;
    let nodes: Vec<_> = ids
        .iter()
        .copied()
        .chain(stand_ins.iter().map(|(id, _)| *id))
        .zip(reaches)
        .collect();
    partition(&nodes, idle_entities, players)
}

/// Splits `nodes` (ticking entities in list order) into islands. Idle entities and players only
/// connect islands.
/// Returns `None` when the tick needs serial execution or planning exceeds its budget.
#[must_use]
pub fn partition(
    nodes: &[(i32, TickReach)],
    idle_entities: &[IdleEntity],
    players: &[(i32, BoundingBox)],
) -> Option<Islands> {
    if nodes.iter().any(|(_, reach)| reach.global) {
        return None;
    }
    let bounds: Vec<_> = nodes
        .iter()
        .map(|(id, reach)| (*id, total_bounds(reach)))
        .collect();
    // Groups can't touch each other, so each one is split on its own and in parallel.
    let parts: Vec<_> = groups(&bounds, |slot| &nodes[slot].1.links, idle_entities, players)
        .par_iter()
        .map(|group| partition_group(nodes, idle_entities, players, group))
        .collect::<Option<_>>()?;
    Some(merge(parts))
}

/// Joins the islands of separate groups.
fn merge(parts: Vec<Islands>) -> Islands {
    let mut islands = Vec::new();
    #[cfg(debug_assertions)]
    let mut island_of = IslandMap::default();
    for part in parts {
        #[cfg(debug_assertions)]
        island_of.extend(
            part.island_of
                .iter()
                .map(|(id, island)| (*id, island + islands.len())),
        );
        islands.extend(part.islands);
    }
    Islands {
        islands,
        #[cfg(debug_assertions)]
        island_of: Arc::new(island_of),
    }
}

/// Everything a reach can touch, as one box.
fn total_bounds(reach: &TickReach) -> BoundingBox {
    let mut bounds = union(&union(&reach.body, &reach.entities), &reach.players);
    for (area, _) in &reach.typed_entities {
        bounds = union(&bounds, area);
    }
    bounds = union(&bounds, &reach.block_reads);
    if let Some(writes) = reach.block_writes {
        bounds = union(&bounds, &writes);
    }
    bounds
}

/// Nodes, idle entities and players that may touch each other, by index.
struct Group {
    nodes: Vec<usize>,
    idle_entities: Vec<usize>,
    players: Vec<usize>,
}

/// Joins everything whose combined bounds share a coarse cell or a link. This only ever joins
/// more than `partition_group` does, so it never separates entities that interact.
fn groups<'a>(
    nodes: &[(i32, BoundingBox)],
    links: impl Fn(usize) -> &'a [i32],
    idle_entities: &[IdleEntity],
    players: &[(i32, BoundingBox)],
) -> Vec<Group> {
    let whole = || {
        vec![Group {
            nodes: (0..nodes.len()).collect(),
            idle_entities: (0..idle_entities.len()).collect(),
            players: (0..players.len()).collect(),
        }]
    };
    let first_idle = nodes.len();
    let first_player = first_idle + idle_entities.len();
    let mut sets = DisjointSets::new(first_player + players.len());
    let mut cells: FxHashMap<(i32, i32), usize> = FxHashMap::default();
    let mut add = |sets: &mut DisjointSets, slot: usize, area: &BoundingBox| -> Option<()> {
        let xs = cells_of(area.min.x, area.max.x, GROUP_CELL_SIZE)?;
        let zs = cells_of(area.min.z, area.max.z, GROUP_CELL_SIZE)?;
        if cell_count(&xs)?.checked_mul(cell_count(&zs)?)? > MAX_GROUP_CELLS {
            return None;
        }
        for x in xs {
            for z in zs.clone() {
                let first = *cells.entry((x, z)).or_insert(slot);
                sets.union(slot, first);
            }
        }
        Some(())
    };
    let mut slots: FxHashMap<i32, usize> = FxHashMap::default();
    for (slot, (id, bounds)) in nodes.iter().enumerate() {
        if add(&mut sets, slot, bounds).is_none() {
            return whole();
        }
        slots.insert(*id, slot);
    }
    for (offset, (id, body, _)) in idle_entities.iter().enumerate() {
        if add(&mut sets, first_idle + offset, body).is_none() {
            return whole();
        }
        slots.insert(*id, first_idle + offset);
    }
    for (offset, (id, body)) in players.iter().enumerate() {
        if add(&mut sets, first_player + offset, body).is_none() {
            return whole();
        }
        slots.insert(*id, first_player + offset);
    }
    for slot in 0..nodes.len() {
        for link in links(slot) {
            let other = *slots.entry(*link).or_insert_with(|| sets.push());
            sets.union(slot, other);
        }
    }

    let mut group_of_root: FxHashMap<usize, usize> = FxHashMap::default();
    let mut groups: Vec<Group> = Vec::new();
    for slot in 0..nodes.len() {
        let group = *group_of_root.entry(sets.find(slot)).or_insert_with(|| {
            groups.push(Group {
                nodes: Vec::new(),
                idle_entities: Vec::new(),
                players: Vec::new(),
            });
            groups.len() - 1
        });
        groups[group].nodes.push(slot);
    }
    // Idle entities and players that share no group with a node can't connect islands.
    for offset in 0..idle_entities.len() {
        if let Some(&group) = group_of_root.get(&sets.find(first_idle + offset)) {
            groups[group].idle_entities.push(offset);
        }
    }
    for offset in 0..players.len() {
        if let Some(&group) = group_of_root.get(&sets.find(first_player + offset)) {
            groups[group].players.push(offset);
        }
    }
    groups
}

/// The bodies of a group's nodes, idle entities and players.
struct Bodies<'a> {
    /// Nodes, then idle entities.
    grid: &'a Grid,
    kinds: &'a [Option<&'static EntityType>],
    slots: &'a FxHashMap<i32, usize>,
    players: &'a [(i32, BoundingBox)],
    first_player: usize,
}

impl Bodies<'_> {
    fn of(&self, id: i32) -> Option<BoundingBox> {
        let slot = *self.slots.get(&id)?;
        if slot < self.first_player {
            Some(self.grid.boxes[slot])
        } else {
            self.players
                .get(slot - self.first_player)
                .map(|(_, body)| *body)
        }
    }

    /// Visits the grid slots `reach`'s entity and typed searches can touch.
    fn reached_by(
        &self,
        query: &mut Query,
        reach: &TickReach,
        budget: &mut PlanningBudget,
        mut visit: impl FnMut(usize),
    ) -> Option<()> {
        self.grid
            .for_each_overlapping(query, &reach.entities, budget, &mut visit)?;
        for (area, target) in &reach.typed_entities {
            self.grid
                .for_each_overlapping(query, area, budget, |member| {
                    if self.kinds[member].is_none_or(|kind| target.matches(kind)) {
                        visit(member);
                    }
                })?;
        }
        Some(())
    }
}

/// Each node's block reads. A sight line runs between two bodies, so the box spanning both
/// holds it. A node that spans its contacts also spans the entities it reaches, those that
/// reach it, and their links.
fn read_areas(
    nodes: &[&(i32, TickReach)],
    bodies: &Bodies,
    budget: &mut PlanningBudget,
) -> Option<Vec<BoundingBox>> {
    let reaches: Vec<&TickReach> = nodes.iter().map(|(_, reach)| reach).collect();
    let flagged: Vec<usize> = (0..reaches.len())
        .filter(|&slot| reaches[slot].spans_contacts)
        .collect();
    let mut contacts: Vec<Vec<usize>> = vec![Vec::new(); reaches.len()];
    if !flagged.is_empty() {
        let mut flagged_grid = Grid::default();
        let mut flagged_kinds = Vec::with_capacity(flagged.len());
        let mut flagged_slots = FxHashMap::default();
        for (index, &slot) in flagged.iter().enumerate() {
            flagged_grid.insert(index, reaches[slot].body, budget)?;
            flagged_kinds.push(reaches[slot].kind);
            flagged_slots.insert(nodes[slot].0, index);
        }
        let flagged_bodies = Bodies {
            grid: &flagged_grid,
            kinds: &flagged_kinds,
            slots: &flagged_slots,
            players: &[],
            first_player: flagged.len(),
        };
        let mut query = Query::new(flagged.len());
        for (slot, reach) in reaches.iter().enumerate() {
            let mut found = Vec::new();
            flagged_bodies.reached_by(&mut query, reach, budget, |index| found.push(index))?;
            found.extend(reach.links.iter().filter_map(|id| flagged_slots.get(id)));
            for index in found {
                if flagged[index] != slot {
                    contacts[flagged[index]].push(slot);
                }
            }
        }
        let mut query = Query::new(bodies.grid.boxes.len());
        for &slot in &flagged {
            let mut found = Vec::new();
            bodies.reached_by(&mut query, reaches[slot], budget, |member| {
                found.push(member);
            })?;
            contacts[slot].extend(found.into_iter().filter(|&member| member != slot));
        }
    }
    let mut areas = Vec::with_capacity(reaches.len());
    for (slot, reach) in reaches.iter().enumerate() {
        let mut spanned: Vec<BoundingBox> =
            reach.sight.iter().filter_map(|id| bodies.of(*id)).collect();
        for &member in &contacts[slot] {
            spanned.push(bodies.grid.boxes[member]);
            if let Some(other) = reaches.get(member) {
                spanned.extend(other.links.iter().filter_map(|id| bodies.of(*id)));
            }
        }
        budget.spend(spanned.len())?;
        areas.push(
            spanned
                .iter()
                .fold(reach.block_reads, |reads, body| union(&reads, body)),
        );
    }
    Some(areas)
}

/// Joins each block writer with the nodes reading blocks it can change.
fn union_block_conflicts(
    sets: &mut DisjointSets,
    reads: Vec<BoundingBox>,
    writers: Vec<(usize, BoundingBox)>,
    budget: &mut PlanningBudget,
) -> Option<()> {
    let mut block_grid = Grid::default();
    for (slot, reads) in reads.into_iter().enumerate() {
        block_grid.insert(slot, reads, budget)?;
    }
    let mut query = Query::new(sets.len());
    for (slot, writes) in writers {
        block_grid.union_overlapping(sets, &mut query, slot, &writes, budget)?;
    }
    Some(())
}

/// Joins `slot` with the bodies in `grid` whose kind its typed searches match.
fn union_typed(
    grid: &Grid,
    kinds: &[Option<&'static EntityType>],
    sets: &mut DisjointSets,
    query: &mut Query,
    slot: usize,
    reach: &TickReach,
    budget: &mut PlanningBudget,
) -> Option<()> {
    for (area, target) in &reach.typed_entities {
        grid.for_each_overlapping(query, area, budget, |member| {
            if kinds[member].is_none_or(|kind| target.matches(kind)) {
                sets.union(slot, member);
            }
        })?;
    }
    Some(())
}

fn partition_group(
    nodes: &[(i32, TickReach)],
    idle_entities: &[IdleEntity],
    players: &[(i32, BoundingBox)],
    group: &Group,
) -> Option<Islands> {
    let node = |slot: usize| &nodes[group.nodes[slot]];
    let players: Vec<_> = group.players.iter().map(|&index| players[index]).collect();
    let mut budget = PlanningBudget::default();
    budget.spend(group.nodes.len())?;
    budget.spend(group.idle_entities.len())?;
    budget.spend(players.len())?;

    let mut sets = DisjointSets::new(group.nodes.len() + group.idle_entities.len() + players.len());
    let mut slots: FxHashMap<i32, usize> = FxHashMap::default();
    let bodies = (0..group.nodes.len())
        .map(|slot| (node(slot).0, node(slot).1.body, node(slot).1.kind))
        .chain(
            group
                .idle_entities
                .iter()
                .map(|&index| idle_entities[index])
                .map(|(id, body, kind)| (id, body, Some(kind))),
        );
    let mut entity_grid = Grid::default();
    let mut kinds = Vec::with_capacity(group.nodes.len() + group.idle_entities.len());
    for (slot, (id, body, kind)) in bodies.enumerate() {
        slots.insert(id, slot);
        entity_grid.insert(slot, body, &mut budget)?;
        kinds.push(kind);
    }
    let first_player = group.nodes.len() + group.idle_entities.len();
    for (offset, (id, _)) in players.iter().enumerate() {
        slots.insert(*id, first_player + offset);
    }

    let mut query = Query::new(sets.len());
    for slot in 0..group.nodes.len() {
        let reach = &node(slot).1;
        entity_grid.union_overlapping(&mut sets, &mut query, slot, &reach.entities, &mut budget)?;
        union_typed(
            &entity_grid,
            &kinds,
            &mut sets,
            &mut query,
            slot,
            reach,
            &mut budget,
        )?;
        budget.spend(players.len())?;
        for (offset, (_, body)) in players.iter().enumerate() {
            if overlaps(&reach.players, body) {
                sets.union(slot, first_player + offset);
            }
        }
        budget.spend(reach.links.len())?;
        for link in &reach.links {
            let other = *slots.entry(*link).or_insert_with(|| sets.push());
            sets.union(slot, other);
        }
    }

    let writers: Vec<_> = (0..group.nodes.len())
        .filter_map(|slot| node(slot).1.block_writes.map(|writes| (slot, writes)))
        .collect();
    if !writers.is_empty() {
        let reaches: Vec<_> = (0..group.nodes.len()).map(node).collect();
        let bodies = Bodies {
            grid: &entity_grid,
            kinds: &kinds,
            slots: &slots,
            players: &players,
            first_player,
        };
        let reads = read_areas(&reaches, &bodies, &mut budget)?;
        union_block_conflicts(&mut sets, reads, writers, &mut budget)?;
    }

    let mut island_of_root: FxHashMap<usize, usize> = FxHashMap::default();
    let mut islands: Vec<Vec<usize>> = Vec::new();
    for slot in 0..group.nodes.len() {
        let root = sets.find(slot);
        let island = *island_of_root.entry(root).or_insert_with(|| {
            islands.push(Vec::new());
            islands.len() - 1
        });
        islands[island].push(group.nodes[slot]);
    }
    #[cfg(debug_assertions)]
    let island_of = slots
        .into_iter()
        .filter_map(|(id, slot)| {
            island_of_root
                .get(&sets.find(slot))
                .map(|island| (id, *island))
        })
        .collect();
    Some(Islands {
        islands,
        #[cfg(debug_assertions)]
        island_of: Arc::new(island_of),
    })
}

#[cfg(debug_assertions)]
thread_local! {
    static TICKING: std::cell::RefCell<Option<(i32, Arc<IslandMap>)>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(debug_assertions)]
static REPORTED_ESCAPES: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Marks `entity_id` as ticking on this thread until [`leave`].
#[cfg(debug_assertions)]
pub fn enter(islands: &Islands, entity_id: i32) {
    TICKING.set(Some((entity_id, islands.island_of.clone())));
}

#[cfg(debug_assertions)]
pub fn leave() {
    TICKING.set(None);
}

/// Warns in debug builds when the ticking entity reaches `entity_id` in another island.
#[cfg_attr(
    not(debug_assertions),
    expect(
        clippy::missing_const_for_fn,
        reason = "The debug implementation accesses thread-local state"
    )
)]
pub fn check_reached(entity_id: i32) {
    #[cfg(debug_assertions)]
    {
        let escaped = TICKING.with_borrow(|ticking| {
            ticking.as_ref().and_then(|(ticking, island_of)| {
                let own = island_of.get(ticking)?;
                let other = island_of.get(&entity_id)?;
                (own != other).then_some(*ticking)
            })
        });
        if let Some(ticking) = escaped
            && REPORTED_ESCAPES.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 20
        {
            tracing::warn!(
                "Entity {ticking} reached entity {entity_id} outside its tick island; its tick reach is too small"
            );
        }
    }
    #[cfg(not(debug_assertions))]
    let _ = entity_id;
}

fn is_large(area: &BoundingBox) -> bool {
    area.max.x - area.min.x > LARGE_CELL_SIZE || area.max.z - area.min.z > LARGE_CELL_SIZE
}

/// Touching faces count as overlapping.
fn overlaps(a: &BoundingBox, b: &BoundingBox) -> bool {
    a.min.x <= b.max.x
        && a.max.x >= b.min.x
        && a.min.y <= b.max.y
        && a.max.y >= b.min.y
        && a.min.z <= b.max.z
        && a.max.z >= b.min.z
}

fn cells_of(min: f64, max: f64, size: f64) -> Option<std::ops::RangeInclusive<i32>> {
    if !min.is_finite() || !max.is_finite() || min > max {
        return None;
    }
    Some(((min / size).floor() as i32)..=((max / size).floor() as i32))
}

fn cell_count(range: &std::ops::RangeInclusive<i32>) -> Option<usize> {
    usize::try_from(i64::from(*range.end()) - i64::from(*range.start()) + 1).ok()
}

struct Cell {
    members: Vec<usize>,
    lowest_top: f64,
    highest_bottom: f64,
    /// Every member is already in one set.
    merged: bool,
}

/// Boxes bucketed by column. Slots are inserted in order from zero.
#[derive(Default)]
struct Grid {
    refined: bool,
    small_cells: FxHashMap<(i32, i32), Cell>,
    cells: FxHashMap<(i32, i32), Cell>,
    large_cells: FxHashMap<(i32, i32), Cell>,
    boxes: Vec<BoundingBox>,
}

impl Grid {
    fn insert(
        &mut self,
        slot: usize,
        area: BoundingBox,
        budget: &mut PlanningBudget,
    ) -> Option<()> {
        debug_assert_eq!(slot, self.boxes.len());
        self.insert_cells(slot, area, budget)?;
        self.boxes.push(area);
        if !self.refined
            && !self.cells.is_empty()
            && self.boxes.len() / MIN_COLUMN_DENSITY >= self.cells.len()
        {
            // Sparse grids retain the cheaper coarse-only query path.
            self.refined = true;
            self.cells.clear();
            for slot in 0..self.boxes.len() {
                if !is_large(&self.boxes[slot]) {
                    self.insert_cells(slot, self.boxes[slot], budget)?;
                }
            }
        }
        Some(())
    }

    fn insert_cells(
        &mut self,
        slot: usize,
        area: BoundingBox,
        budget: &mut PlanningBudget,
    ) -> Option<()> {
        let (cells, size) = if is_large(&area) {
            (&mut self.large_cells, LARGE_CELL_SIZE)
        } else if self.refined
            && area.max.x - area.min.x <= SMALL_CELL_SIZE
            && area.max.z - area.min.z <= SMALL_CELL_SIZE
        {
            (&mut self.small_cells, SMALL_CELL_SIZE)
        } else {
            (&mut self.cells, CELL_SIZE)
        };
        let [xs, zs] = budget.cells(&area, size)?;
        for x in xs {
            for z in zs.clone() {
                let cell = cells.entry((x, z)).or_insert_with(|| Cell {
                    members: Vec::new(),
                    lowest_top: f64::INFINITY,
                    highest_bottom: f64::NEG_INFINITY,
                    merged: false,
                });
                cell.members.push(slot);
                cell.lowest_top = cell.lowest_top.min(area.max.y);
                cell.highest_bottom = cell.highest_bottom.max(area.min.y);
            }
        }
        Some(())
    }

    fn for_each_overlapping(
        &self,
        query: &mut Query,
        area: &BoundingBox,
        budget: &mut PlanningBudget,
        mut visit: impl FnMut(usize),
    ) -> Option<()> {
        query.next();
        for (cells, size) in [
            (&self.small_cells, SMALL_CELL_SIZE),
            (&self.cells, CELL_SIZE),
            (&self.large_cells, LARGE_CELL_SIZE),
        ] {
            if cells.is_empty() {
                continue;
            }
            let [xs, zs] = budget.cells(area, size)?;
            for x in xs {
                for z in zs.clone() {
                    if let Some(cell) = cells.get(&(x, z)) {
                        budget.spend(cell.members.len())?;
                        for &member in &cell.members {
                            if query.visit(member) && overlaps(area, &self.boxes[member]) {
                                visit(member);
                            }
                        }
                    }
                }
            }
        }
        Some(())
    }

    fn union_overlapping(
        &mut self,
        sets: &mut DisjointSets,
        query: &mut Query,
        slot: usize,
        area: &BoundingBox,
        budget: &mut PlanningBudget,
    ) -> Option<()> {
        query.next();
        for (cells, size) in [
            (&mut self.small_cells, SMALL_CELL_SIZE),
            (&mut self.cells, CELL_SIZE),
            (&mut self.large_cells, LARGE_CELL_SIZE),
        ] {
            if cells.is_empty() {
                continue;
            }
            let [xs, zs] = budget.cells(area, size)?;
            for x in xs {
                for z in zs.clone() {
                    let Some(cell) = cells.get_mut(&(x, z)) else {
                        continue;
                    };
                    let (min_x, min_z) = (f64::from(x) * size, f64::from(z) * size);
                    // `area` spans the column and every member's height, so all members overlap it.
                    let covered = area.min.x <= min_x
                        && area.max.x >= min_x + size
                        && area.min.z <= min_z
                        && area.max.z >= min_z + size
                        && area.min.y <= cell.lowest_top
                        && area.max.y >= cell.highest_bottom;
                    if covered {
                        if cell.merged {
                            budget.spend(1)?;
                            sets.union(slot, cell.members[0]);
                        } else {
                            budget.spend(cell.members.len())?;
                            for &member in &cell.members {
                                sets.union(slot, member);
                            }
                            cell.merged = true;
                        }
                        continue;
                    }
                    if cell.merged {
                        // One overlapping member joins the whole cell.
                        if sets.find(cell.members[0]) == sets.find(slot) {
                            budget.spend(1)?;
                            continue;
                        }
                        for &member in &cell.members {
                            budget.spend(1)?;
                            if overlaps(area, &self.boxes[member]) {
                                sets.union(slot, member);
                                break;
                            }
                        }
                        continue;
                    }
                    budget.spend(cell.members.len())?;
                    let mut joined = true;
                    for &member in &cell.members {
                        if query.visit(member) && overlaps(area, &self.boxes[member]) {
                            sets.union(slot, member);
                        } else if sets.find(member) != sets.find(slot) {
                            joined = false;
                        }
                    }
                    cell.merged = joined;
                }
            }
        }
        Some(())
    }
}

/// Visits each member once per query.
struct Query {
    stamps: Vec<u32>,
    current: u32,
}

impl Query {
    fn new(len: usize) -> Self {
        Self {
            stamps: vec![0; len],
            current: 0,
        }
    }

    const fn next(&mut self) {
        self.current += 1;
    }

    fn visit(&mut self, member: usize) -> bool {
        let stamp = &mut self.stamps[member];
        let first = *stamp != self.current;
        *stamp = self.current;
        first
    }
}

struct DisjointSets {
    parent: Vec<usize>,
}

impl DisjointSets {
    fn new(len: usize) -> Self {
        Self {
            parent: (0..len).collect(),
        }
    }

    const fn len(&self) -> usize {
        self.parent.len()
    }

    fn push(&mut self) -> usize {
        self.parent.push(self.parent.len());
        self.parent.len() - 1
    }

    fn find(&mut self, mut index: usize) -> usize {
        while self.parent[index] != index {
            self.parent[index] = self.parent[self.parent[index]];
            index = self.parent[index];
        }
        index
    }

    fn union(&mut self, a: usize, b: usize) {
        let a = self.find(a);
        let b = self.find(b);
        if a != b {
            // Keep the lower index as root so it does not depend on union order.
            self.parent[a.max(b)] = a.min(b);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pumpkin_util::math::vector3::Vector3;

    #[test]
    fn island_jobs_group_small_islands_on_large_pools() {
        let islands: Vec<Vec<usize>> = [5, 3, 1, 1, 1].map(|len| vec![0; len]).into();
        assert_eq!(island_jobs(&islands, 8), [0..1, 1..2, 2..3, 3..4, 4..5]);
        assert_eq!(island_jobs(&islands, 40), [0..1, 1..3, 3..5]);
    }

    #[test]
    fn serial_schedule_retries_after_twenty_ticks_or_a_large_count_change() {
        let cost = Duration::from_micros(64);
        let mut schedule = Schedule::default();
        assert!(!schedule.should_plan(1));
        assert!(schedule.should_plan(64));
        assert!(!schedule.record_plan(64, cost));
        // Small spawns and despawns keep the serial preference.
        for tick in 0..19 {
            assert!(!schedule.should_plan(64 + tick % 9));
        }
        assert!(schedule.should_plan(64));
        assert!(!schedule.record_plan(48, cost));
        assert!(!schedule.should_plan(56));
        assert!(schedule.should_plan(73));
        assert!(schedule.record_plan(49, cost));
        // The first parallel tick is followed by a serial one to compare with.
        assert!(!schedule.should_plan(73));
        schedule.record_serial(Duration::from_micros(2 * 73));
        // Parallel ticks always plan.
        assert!(schedule.should_plan(73));
        assert!(schedule.record_plan(49, cost));
        assert!(schedule.should_plan(70));
        assert!(!schedule.record_plan(70, cost));
        assert!(!schedule.should_plan(1));
        assert!(schedule.should_plan(65));
        assert!(schedule.record_plan(48, cost));
        assert!(schedule.should_plan(65));
    }

    #[test]
    fn planned_ticks_slower_than_serial_ones_cache_the_serial_preference() {
        let planned = Duration::from_micros(800);
        let mut schedule = Schedule::default();
        assert!(schedule.should_plan(40));
        assert!(schedule.record_plan(8, planned));
        assert!(!schedule.should_plan(40));
        schedule.record_serial(Duration::from_micros(600));
        assert!(schedule.should_plan(40));
        assert!(!schedule.record_plan(8, planned));
        for _ in 0..19 {
            assert!(!schedule.should_plan(40));
            schedule.record_serial(Duration::from_micros(1200));
        }
        // Serial ticks got slower, so planning wins again.
        assert!(schedule.should_plan(40));
        assert!(schedule.record_plan(8, planned));
        // Twenty parallel ticks in a row measure a serial one again.
        for _ in 0..19 {
            assert!(schedule.should_plan(40));
            assert!(schedule.record_plan(8, planned));
        }
        assert!(!schedule.should_plan(40));
    }

    #[expect(
        clippy::expect_used,
        reason = "Ordinary fixtures must fit the planning budget."
    )]
    fn islands(
        nodes: &[(i32, TickReach)],
        idle_entities: &[IdleEntity],
        players: &[(i32, BoundingBox)],
    ) -> Vec<Vec<usize>> {
        partition(nodes, idle_entities, players)
            .expect("ordinary fixture exceeded the planning budget")
            .islands
    }

    fn cube(x: f64, size: f64) -> BoundingBox {
        BoundingBox::new(
            Vector3::new(x, 64.0, 0.0),
            Vector3::new(x + size, 64.0 + size, size),
        )
    }

    fn node(id: i32, x: f64, range: f64) -> (i32, TickReach) {
        let body = cube(x, 1.0);
        let area = body.expand(range, range, range);
        (
            id,
            TickReach {
                body,
                entities: area,
                players: area,
                typed_entities: Vec::new(),
                kind: Some(&EntityType::PIG),
                block_reads: body,
                block_writes: None,
                global: false,
                links: Vec::new(),
                sight: Vec::new(),
                spans_contacts: false,
            },
        )
    }

    #[test]
    fn incremental_grid_queries_match_all_inserted_boxes() -> Result<(), &'static str> {
        let mut grid = Grid::default();
        let mut query = Query::new(128);
        let mut budget = PlanningBudget::default();
        for slot in 0..128 {
            let min = if slot < 32 {
                Vector3::new(
                    f64::from(slot % 8) * 0.5 - 8.0,
                    f64::from(slot % 3) * 16.0,
                    f64::from(slot / 8) * 0.5 - 8.0,
                )
            } else {
                Vector3::new(
                    f64::from(slot % 19) * 4.0 - 36.0,
                    f64::from(slot % 5) * 16.0,
                    f64::from(slot / 19) * 4.0 - 16.0,
                )
            };
            let size = if slot == 0 {
                4.25
            } else if slot < 32 {
                0.25
            } else {
                [0.0, 0.25, 4.0, 4.25, 17.0][slot as usize % 5]
            };
            let mut body = BoundingBox::new(min, min + Vector3::new(size, 2.0, size));
            if slot % 9 == 0 {
                body.min.y = f64::NEG_INFINITY;
                body.max.y = f64::INFINITY;
            }
            grid.insert(slot as usize, body, &mut budget)
                .ok_or("grid budget exhausted")?;
            for area in [body, body.expand(16.0, 0.0, 16.0)] {
                let expected: Vec<_> = grid
                    .boxes
                    .iter()
                    .enumerate()
                    .filter_map(|(slot, body)| overlaps(&area, body).then_some(slot))
                    .collect();
                let mut found = Vec::new();
                grid.for_each_overlapping(&mut query, &area, &mut budget, |slot| found.push(slot))
                    .ok_or("query budget exhausted")?;
                found.sort_unstable();
                assert_eq!(found, expected);
            }
        }
        Ok(())
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "one randomized fixture checked against a brute-force reference"
    )]
    fn partition_matches_pairwise_connected_components() {
        use rand::{RngExt, SeedableRng, rngs::StdRng};

        fn bounds(random: &mut StdRng) -> BoundingBox {
            let min = Vector3::new(
                f64::from(random.random_range(-16..16)) * 4.0,
                f64::from(random.random_range(-4..4)) * 16.0,
                f64::from(random.random_range(-16..16)) * 4.0,
            );
            let size = [0.25, 4.0, 4.25, 16.0, 32.0][random.random_range(0..5)];
            BoundingBox::new(min, min + Vector3::new(size, 2.0, size))
        }

        let mut random = StdRng::seed_from_u64(42);
        for _ in 0..32 {
            let nodes: Vec<_> = (0..64)
                .map(|id| {
                    let body = if id < 48 {
                        let min = Vector3::new(
                            f64::from(id % 6) * 0.5,
                            f64::from(id / 24) * 16.0,
                            f64::from(id / 6) * 0.5,
                        );
                        BoundingBox::new(min, min + Vector3::new(0.25, 2.0, 0.25))
                    } else {
                        bounds(&mut random)
                    };
                    let mut reads = bounds(&mut random);
                    reads.min.y = f64::NEG_INFINITY;
                    reads.max.y = f64::INFINITY;
                    (
                        id,
                        TickReach {
                            body,
                            entities: body.expand(0.5, 0.5, 0.5),
                            players: body.expand(4.0, 4.0, 4.0),
                            typed_entities: Vec::new(),
                            kind: Some(&EntityType::PIG),
                            block_reads: reads,
                            block_writes: (id % 4 == 0).then(|| bounds(&mut random)),
                            global: false,
                            links: if id % 7 == 0 { vec![1000] } else { Vec::new() },
                            sight: Vec::new(),
                            spans_contacts: false,
                        },
                    )
                })
                .collect();
            let idle: Vec<_> = (100..104)
                .map(|id| (id, bounds(&mut random), &EntityType::PIG))
                .collect();
            let players: Vec<_> = (200..203).map(|id| (id, bounds(&mut random))).collect();
            let bodies: Vec<_> = nodes
                .iter()
                .map(|(id, reach)| (*id, reach.body))
                .chain(idle.iter().map(|(id, body, _)| (*id, *body)))
                .chain(players.iter().copied())
                .collect();
            // The extra vertex represents an external link shared by several nodes.
            let mut edges = vec![vec![false; bodies.len() + 1]; bodies.len() + 1];
            for (slot, (_, reach)) in nodes.iter().enumerate() {
                for (other, (_, body)) in bodies.iter().enumerate() {
                    let area = if other < nodes.len() + idle.len() {
                        &reach.entities
                    } else {
                        &reach.players
                    };
                    if overlaps(area, body) {
                        edges[slot][other] = true;
                        edges[other][slot] = true;
                    }
                }
                if !reach.links.is_empty() {
                    edges[slot][bodies.len()] = true;
                    edges[bodies.len()][slot] = true;
                }
                if let Some(writes) = reach.block_writes {
                    for (other, (_, reader)) in nodes.iter().enumerate() {
                        if overlaps(&writes, &reader.block_reads) {
                            edges[slot][other] = true;
                            edges[other][slot] = true;
                        }
                    }
                }
            }
            let mut visited = vec![false; edges.len()];
            let mut expected = Vec::new();
            for start in 0..nodes.len() {
                if visited[start] {
                    continue;
                }
                let mut pending = VecDeque::from([start]);
                let mut component = Vec::new();
                visited[start] = true;
                while let Some(slot) = pending.pop_front() {
                    if slot < nodes.len() {
                        component.push(slot);
                    }
                    for (other, connected) in edges[slot].iter().enumerate() {
                        if *connected && !visited[other] {
                            visited[other] = true;
                            pending.push_back(other);
                        }
                    }
                }
                component.sort_unstable();
                expected.push(component);
            }
            assert_eq!(islands(&nodes, &idle, &players), expected);
        }
    }

    #[test]
    fn entities_out_of_reach_tick_apart_in_original_order() {
        let nodes = [
            node(10, 0.0, 0.5),
            node(11, 100.0, 0.5),
            node(12, 1.5, 0.5),
            node(13, 200.0, 0.5),
        ];
        assert_eq!(islands(&nodes, &[], &[]), [vec![0, 2], vec![1], vec![3]]);
    }

    #[test]
    fn one_sided_reach_joins_both_entities() {
        let nodes = [node(10, 0.0, 30.0), node(11, 25.0, 0.0)];
        assert_eq!(islands(&nodes, &[], &[]), [vec![0, 1]]);
    }

    #[tokio::test]
    async fn detonating_tnt_preserves_world_tick_order() -> Result<(), Box<dyn std::error::Error>> {
        use crate::entity::{Entity, tnt::TNTEntity};
        use pumpkin_nbt::compound::NbtCompound;

        let folder = tempfile::tempdir()?;
        let world = crate::entity::ai::test_support::world(&folder);
        let tnt = TNTEntity::new(
            Entity::new(
                world.clone(),
                Vector3::new(0.0, 64.0, 0.0),
                &EntityType::TNT,
            ),
            TNTEntity::DEFAULT_POWER,
            TNTEntity::DEFAULT_FUSE,
        );
        let config = EntityIslandsConfig::default();
        for fuse in [2, 1, 0] {
            let mut nbt = NbtCompound::new();
            nbt.put_short("fuse", fuse);
            nbt.put_float("explosion_power", 128.0);
            tnt.read_custom_nbt(&nbt);
            let reaches = resolve(vec![EntityReach::of(&tnt, &config)], &config);
            if fuse <= 1 {
                assert!(reaches.is_none());
                continue;
            }
            let reaches = reaches.ok_or("unexpected serial fallback")?;
            let nodes = [
                (tnt.get_entity().entity_id, reaches[0].clone()),
                node(-1, 150.0, 0.0),
                node(-2, 400.0, 0.0),
            ];
            assert_eq!(islands(&nodes, &[], &[]), [vec![0], vec![1], vec![2]]);
        }
        world.level.shutdown().await;
        Ok(())
    }

    #[tokio::test]
    async fn unaudited_living_motion_joins_entities_at_the_destination()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::entity::{Entity, passive::polar_bear::PolarBearEntity};

        let folder = tempfile::tempdir()?;
        let world = crate::entity::ai::test_support::world(&folder);
        let bear = PolarBearEntity::new(Entity::new(
            world.clone(),
            Vector3::new(0.0, 64.0, 0.0),
            &EntityType::POLAR_BEAR,
        ));
        assert!(bear.audited_reach().is_none());
        let config = EntityIslandsConfig::default();
        for speed in [0.0, -96.0, 96.0] {
            bear.get_entity()
                .velocity
                .store(Vector3::new(speed, 0.0, 0.0));
            let reaches = resolve(vec![EntityReach::of(bear.as_ref(), &config)], &config)
                .ok_or("unexpected serial fallback")?;
            let nodes = [
                (bear.get_entity().entity_id, reaches[0].clone()),
                node(-1, 96.0, 0.0),
                node(-2, -96.0, 0.0),
            ];
            if speed == 0.0 {
                assert_eq!(islands(&nodes, &[], &[]), [vec![0], vec![1], vec![2]]);
            } else {
                let destination = if speed > 0.0 { 1 } else { 2 };
                assert!(islands(&nodes, &[], &[])[0].contains(&destination));
            }
        }
        world.level.shutdown().await;
        Ok(())
    }

    #[test]
    fn cells_covered_by_a_large_reach_still_respect_height() {
        let mut low = node(10, 0.0, 0.0);
        low.1.body = BoundingBox::new(Vector3::new(1.0, 0.0, 1.0), Vector3::new(2.0, 1.0, 2.0));
        let mut wide = node(11, 40.0, 0.0);
        wide.1.entities = BoundingBox::new(
            Vector3::new(-64.0, 60.0, -64.0),
            Vector3::new(64.0, 70.0, 64.0),
        );
        assert_eq!(islands(&[low, wide], &[], &[]), [vec![0], vec![1]]);
    }

    #[test]
    fn idle_entities_players_and_links_bridge_islands() {
        let nodes = [node(10, 0.0, 1.0), node(11, 4.0, 1.0)];
        assert_eq!(islands(&nodes, &[], &[]), [vec![0], vec![1]]);
        assert_eq!(
            islands(&nodes, &[(20, cube(2.0, 1.0), &EntityType::PIG)], &[]),
            [vec![0, 1]]
        );
        assert_eq!(islands(&nodes, &[], &[(30, cube(2.0, 1.0))]), [vec![0, 1]]);
        let mut linked = nodes;
        linked[0].1.links.push(40);
        linked[1].1.links.push(40);
        assert_eq!(islands(&linked, &[], &[]), [vec![0, 1]]);
        // Links join entities in different coarse groups.
        let mut far = [node(10, 0.0, 1.0), node(11, 10_000.0, 1.0)];
        far[0].1.links.push(11);
        assert_eq!(islands(&far, &[], &[]), [vec![0, 1]]);
    }

    #[test]
    fn distant_clusters_fit_the_budget_separately() {
        // Together the clusters exceed one planning budget; each fits on its own.
        let nodes: Vec<_> = (0..16)
            .flat_map(|cluster| {
                (0..150).map(move |index| {
                    node(
                        cluster * 150 + index,
                        f64::from(cluster) * 2000.0 + f64::from(index) * 0.1,
                        40.0,
                    )
                })
            })
            .collect();
        let islands = islands(&nodes, &[], &[]);
        assert_eq!(islands.len(), 16);
        assert!(islands.iter().all(|island| island.len() == 150));
    }

    #[test]
    fn block_changes_join_entities_reading_those_blocks() {
        let mut nodes = [node(10, 0.0, 0.0), node(11, 30.0, 0.0)];
        assert_eq!(islands(&nodes, &[], &[]), [vec![0], vec![1]]);
        nodes[0].1.block_writes = Some(nodes[0].1.body.expand(17.0, 17.0, 17.0));
        nodes[1].1.block_reads = nodes[1].1.body.expand(10.0, 10.0, 10.0);
        assert_eq!(islands(&nodes, &[], &[]), [vec![0], vec![1]]);
        nodes[1].1.block_reads = nodes[1].1.body.expand(15.0, 15.0, 15.0);
        assert_eq!(islands(&nodes, &[], &[]), [vec![0, 1]]);
    }

    #[test]
    fn dense_block_writers_fit_the_budget() {
        // Like a crowd of unaudited mobs: wide block writes over full-height reads.
        let nodes: Vec<_> = (0..150)
            .map(|id| {
                let (id, mut reach) = node(id, f64::from(id) * 0.5, 0.0);
                reach.block_reads = reach.body.expand(77.0, 0.0, 77.0);
                reach.block_reads.min.y = f64::NEG_INFINITY;
                reach.block_reads.max.y = f64::INFINITY;
                reach.block_writes = Some(reach.body.expand(77.0, 77.0, 77.0));
                (id, reach)
            })
            .collect();
        assert_eq!(islands(&nodes, &[], &[]), [(0..150).collect::<Vec<_>>()]);
    }

    #[tokio::test]
    async fn a_creeper_serializes_the_tick_it_explodes() -> Result<(), Box<dyn std::error::Error>> {
        use crate::entity::{
            Entity,
            mob::{Mob, creeper::CreeperEntity},
        };
        use pumpkin_nbt::compound::NbtCompound;

        let folder = tempfile::tempdir()?;
        let world = crate::entity::ai::test_support::world(&folder);
        let creeper = CreeperEntity::new(Entity::new(
            world.clone(),
            Vector3::new(0.0, 64.0, 0.0),
            &EntityType::CREEPER,
        ));
        // A blast this wide reaches past every margin of the unaudited fallback.
        let mut nbt = NbtCompound::new();
        nbt.put_byte("ExplosionRadius", 100);
        creeper.mob_read_nbt(&nbt);
        let config = EntityIslandsConfig::default();
        assert!(matches!(
            EntityReach::of(creeper.as_ref(), &config),
            EntityReach::Audited(_)
        ));
        creeper.set_ignited(true);
        // Ignited, it primes next tick: a game event outside the audited reach.
        assert!(matches!(
            EntityReach::of(creeper.as_ref(), &config),
            EntityReach::Conservative(_)
        ));
        creeper
            .current_fuse_time
            .store(creeper.get_fuse() - 1, std::sync::atomic::Ordering::Relaxed);
        assert!(resolve(vec![EntityReach::of(creeper.as_ref(), &config)], &config).is_none());
        world.level.shutdown().await;
        Ok(())
    }

    #[test]
    fn typed_searches_join_only_the_kinds_they_match() {
        let golems = TargetMatch::Type(&EntityType::IRON_GOLEM);
        let mut nodes = [node(10, 0.0, 0.0), node(11, 10.0, 0.0)];
        nodes[0].1.typed_entities = vec![(cube(0.0, 1.0).expand(16.0, 16.0, 16.0), golems)];
        assert_eq!(islands(&nodes, &[], &[]), [vec![0], vec![1]]);
        let idle_pig = [(20, cube(5.0, 1.0), &EntityType::PIG)];
        assert_eq!(islands(&nodes, &idle_pig, &[]), [vec![0], vec![1]]);
        // An idle golem in the search box joins nothing else.
        let idle_golem = [(20, cube(5.0, 1.0), &EntityType::IRON_GOLEM)];
        assert_eq!(islands(&nodes, &idle_golem, &[]), [vec![0], vec![1]]);
        nodes[1].1.kind = Some(&EntityType::IRON_GOLEM);
        assert_eq!(islands(&nodes, &[], &[]), [vec![0, 1]]);
        // An unaudited entity can turn into anything during its tick.
        nodes[1].1.kind = None;
        assert_eq!(islands(&nodes, &[], &[]), [vec![0, 1]]);
    }

    #[test]
    fn sight_lines_meet_block_writes_between_the_bodies() {
        let mut nodes = [node(10, 0.0, 0.0), node(11, 40.0, 0.0), node(12, 20.0, 0.0)];
        nodes[0].1.links.push(11);
        nodes[2].1.block_writes = Some(nodes[2].1.body.expand(2.0, 2.0, 2.0));
        assert_eq!(islands(&nodes, &[], &[]), [vec![0, 1], vec![2]]);
        nodes[0].1.sight.push(11);
        assert_eq!(islands(&nodes, &[], &[]), [vec![0, 1, 2]]);
        // An attacker that reaches the mob from afar, which the mob doesn't link in advance.
        let mut nodes = [node(10, 0.0, 0.0), node(11, 40.0, 0.0), node(12, 20.0, 0.0)];
        nodes[1].1.links.push(10);
        nodes[2].1.block_writes = Some(nodes[2].1.body.expand(2.0, 2.0, 2.0));
        assert_eq!(islands(&nodes, &[], &[]), [vec![0, 1], vec![2]]);
        nodes[0].1.spans_contacts = true;
        assert_eq!(islands(&nodes, &[], &[]), [vec![0, 1, 2]]);
    }

    #[test]
    fn audited_hits_throw_entities_that_move_after_them() -> Result<(), &'static str> {
        let config = EntityIslandsConfig::default();
        let attacker = || match audited(0.0) {
            EntityReach::Audited(mut reach) => {
                reach.knockback = 1.0;
                EntityReach::Audited(reach)
            }
            conservative @ EntityReach::Conservative(_) => conservative,
        };
        let still = finish(
            match audited(1.5) {
                EntityReach::Audited(reach) => reach,
                EntityReach::Conservative(_) => return Err("fixture is audited"),
            },
            0.0,
        );
        let before = resolve(vec![audited(1.5), attacker()], &config).ok_or("serial")?;
        assert_box(&before[0].body, &still.body);
        let after = resolve(vec![attacker(), audited(1.5)], &config).ok_or("serial")?;
        assert_box(
            &after[1].body,
            &still.body.expand(
                config.knockback_reach,
                config.knockback_reach,
                config.knockback_reach,
            ),
        );
        Ok(())
    }

    #[test]
    fn a_global_entity_serializes_the_tick() {
        let mut nodes = [node(10, 0.0, 0.0), node(11, 300.0, 0.0)];
        nodes[1].1.global = true;
        assert!(partition(&nodes, &[], &[]).is_none());
    }

    fn assert_box(actual: &BoundingBox, expected: &BoundingBox) {
        assert_eq!((actual.min, actual.max), (expected.min, expected.max));
    }

    fn audited(x: f64) -> EntityReach {
        EntityReach::Audited(AuditedReach {
            bounding_box: cube(x, 1.0),
            motion: (0.25, 0.5),
            entities: 0.0,
            players: 0.0,
            typed_entities: Vec::new(),
            kind: Some(&EntityType::PIG),
            block_reads: 0.0,
            route: None,
            changes_blocks: false,
            global: false,
            links: Vec::new(),
            sight: Vec::new(),
            spans_contacts: false,
            knockback: 0.0,
        })
    }

    #[tokio::test]
    async fn large_contact_scans_request_serial_ticking() -> Result<(), Box<dyn std::error::Error>>
    {
        let folder = tempfile::tempdir()?;
        let world = crate::entity::ai::test_support::world(&folder);
        let EntityReach::Audited(mut reach) = audited(0.0) else {
            return Err("expected audited reach".into());
        };
        for motion in [8.0, f64::INFINITY, f64::NAN] {
            reach.motion = (motion, motion);
            reach.global = false;
            reach.scan_contact_blocks(&world);
            assert!(reach.global);
            assert!(
                resolve(
                    vec![EntityReach::Audited(reach.clone())],
                    &EntityIslandsConfig::default()
                )
                .is_none()
            );
        }
        world.level.shutdown().await;
        Ok(())
    }

    #[tokio::test]
    async fn large_follow_ranges_exhaust_planning_without_changing_attributes()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::entity::{
            Entity,
            mob::{Mob, zombie::zombie::ZombieEntity},
        };

        let folder = tempfile::tempdir()?;
        let world = crate::entity::ai::test_support::world(&folder);
        let zombie = ZombieEntity::new(Entity::new(
            world.clone(),
            Vector3::new(0.0, 64.0, 0.0),
            &EntityType::ZOMBIE,
        ));
        zombie
            .get_mob_entity()
            .living_entity
            .set_attribute_base(&Attributes::FOLLOW_RANGE, 1000.0);
        let config = EntityIslandsConfig::default();
        let reports = (0..256)
            .map(|_| EntityReach::of(zombie.as_ref(), &config))
            .collect();
        assert!(resolve(reports, &config).is_none());
        assert_eq!(
            zombie
                .get_mob_entity()
                .living_entity
                .get_attribute_value(&Attributes::FOLLOW_RANGE),
            1000.0
        );
        world.level.shutdown().await;
        Ok(())
    }

    #[test]
    fn broad_queries_exhaust_the_budget_without_clipping_the_query() -> Result<(), &'static str> {
        let mut grid = Grid::default();
        let mut budget = PlanningBudget::default();
        for slot in 0..64 {
            grid.insert(slot, cube((slot % 8) as f64 * 0.5, 0.25), &mut budget)
                .ok_or("insertion refused")?;
        }
        grid.insert(64, cube(900.0, 1.0), &mut budget)
            .ok_or("distant insertion refused")?;
        assert!(grid.refined);
        let mut query = Query::new(65);
        let area = cube(0.0, 1.0).expand(1000.0, 0.0, 1000.0);
        let mut found = Vec::new();
        grid.for_each_overlapping(&mut query, &area, &mut budget, |slot| found.push(slot))
            .ok_or("first query refused")?;
        found.sort_unstable();
        assert_eq!(found, (0..65).collect::<Vec<_>>());
        assert!(
            grid.for_each_overlapping(&mut query, &area, &mut budget, |_| {})
                .is_none()
        );
        Ok(())
    }

    #[test]
    fn oversized_and_nonfinite_partition_bounds_request_serial_ticking() {
        for bound in [1.0e20, f64::INFINITY, f64::NAN] {
            let mut nodes = [node(0, 0.0, 0.0), node(1, 20.0, 0.0)];
            nodes[0].1.entities = nodes[0].1.entities.expand(bound, 0.0, bound);
            assert!(partition(&nodes, &[], &[]).is_none());
            nodes[0].1.entities = nodes[0].1.body;
            nodes[0].1.block_reads = nodes[0].1.body.expand(bound, 0.0, bound);
            nodes[1].1.block_writes = Some(nodes[1].1.body);
            assert!(partition(&nodes, &[], &[]).is_none());
        }
        let nodes: Vec<_> = (0..2000)
            .map(|id| node(id, f64::from(id) * 4.0, 0.5))
            .collect();
        assert!(partition(&nodes, &[], &[]).is_some());
    }

    #[tokio::test]
    async fn oversized_write_queries_leave_pruning_results_unused()
    -> Result<(), Box<dyn std::error::Error>> {
        let folder = tempfile::tempdir()?;
        let world = crate::entity::ai::test_support::world(&folder);
        world.level.loaded_chunks.insert(
            Vector2::new(0, 0),
            pumpkin_world::chunk::ChunkData::empty_sync(0, 0),
        );
        let mut reaches = vec![node(0, 1.0, 0.0).1, node(1, 32.0, 0.0).1];
        reaches[0].block_writes = Some(reaches[0].body);
        reaches[1].block_writes = Some(reaches[1].body.expand(10_000.0, 0.0, 10_000.0));
        assert!(
            prune_block_writes(
                &world,
                &mut reaches,
                &[BlockWriteCheck::Contact, BlockWriteCheck::Unconditional]
            )
            .is_none()
        );
        assert!(reaches.iter().all(|reach| reach.block_writes.is_some()));
        world.level.shutdown().await;
        Ok(())
    }

    #[tokio::test]
    async fn grouped_planning_matches_planning_the_whole_tick()
    -> Result<(), Box<dyn std::error::Error>> {
        let folder = tempfile::tempdir()?;
        let world = crate::entity::ai::test_support::world(&folder);
        let config = EntityIslandsConfig::default();
        let conservative = |x: f64| {
            EntityReach::Conservative(TickReach::conservative(&cube(x, 1.0), 20.0, Vec::new()))
        };
        let pushed: Vec<_> = (0..16)
            .flat_map(|cluster| {
                let base = f64::from(cluster) * 3000.0;
                [
                    conservative(base),
                    audited(base + 5.0),
                    audited(base + 22.0),
                    audited(base + 200.0),
                ]
            })
            .collect();
        // Past the grouped pusher bound, planning falls back to the whole tick.
        let crowded: Vec<_> = (0..300)
            .map(|_| audited(0.0))
            .chain([audited(5000.0)])
            .collect();
        for reports in [pushed, crowded] {
            let ids: Vec<i32> = (0..reports.len() as i32).collect();
            let checks = vec![BlockWriteCheck::Unconditional; reports.len()];
            let mut grouped = plan(
                &world,
                &ids,
                reports.clone(),
                &checks,
                &[],
                &[],
                &[],
                &config,
            )
            .ok_or("grouped planning refused")?
            .islands;
            let mut reaches = resolve(reports, &config).ok_or("resolve refused")?;
            prune_block_writes(&world, &mut reaches, &checks).ok_or("pruning refused")?;
            let nodes: Vec<_> = ids.iter().copied().zip(reaches).collect();
            let whole = Group {
                nodes: (0..nodes.len()).collect(),
                idle_entities: Vec::new(),
                players: Vec::new(),
            };
            let mut expected = partition_group(&nodes, &[], &[], &whole)
                .ok_or("partition refused")?
                .islands;
            grouped.sort();
            expected.sort();
            assert_eq!(grouped, expected);
        }
        world.level.shutdown().await;
        Ok(())
    }

    #[test]
    fn only_earlier_pushers_grow_current_turn() -> Result<(), &'static str> {
        let alone = resolve(vec![audited(0.0)], &EntityIslandsConfig::default())
            .ok_or("unexpected serial fallback")?;
        assert_box(&alone[0].body, &cube(0.0, 1.0).expand(0.25, 0.5, 0.25));
        let crowded = resolve(
            vec![audited(0.0), audited(0.0), audited(0.0)],
            &EntityIslandsConfig::default(),
        )
        .ok_or("unexpected serial fallback")?;
        assert_box(&crowded[0].body, &alone[0].body);
        assert!(crowded[1].body.min.x < crowded[0].body.min.x);
        assert!(crowded[2].body.min.x < crowded[1].body.min.x);
        assert!(crowded[2].body.max.x > crowded[1].body.max.x);
        Ok(())
    }

    #[test]
    fn unaudited_neighbours_can_throw_audited_entities_around() -> Result<(), &'static str> {
        let zombie =
            EntityReach::Conservative(TickReach::conservative(&cube(10.0, 1.0), 10.0, Vec::new()));
        let reaches = resolve(
            vec![audited(0.0), zombie, audited(40.0)],
            &EntityIslandsConfig::default(),
        )
        .ok_or("unexpected serial fallback")?;
        // The zombie can both hit and push it.
        let thrown = EntityIslandsConfig::default().knockback_reach + PUSH_SPEED;
        assert_box(
            &reaches[0].body,
            &cube(0.0, 1.0).expand(0.25 + thrown, 0.5 + thrown, 0.25 + thrown),
        );
        assert!(reaches[0].block_writes.is_some());
        assert_box(&reaches[2].body, &cube(40.0, 1.0).expand(0.25, 0.5, 0.25));
        Ok(())
    }

    #[tokio::test]
    async fn contact_writers_propagate_through_a_reverse_ordered_chain()
    -> Result<(), Box<dyn std::error::Error>> {
        let folder = tempfile::tempdir()?;
        let world = crate::entity::ai::test_support::world(&folder);
        for x in 0..=4 {
            world.level.loaded_chunks.insert(
                Vector2::new(x, 0),
                pumpkin_world::chunk::ChunkData::empty_sync(x, 0),
            );
        }
        let mut reaches: Vec<_> = [30.0, 20.0, 10.0, 60.0]
            .into_iter()
            .map(|x| node(0, x, 0.0).1)
            .collect();
        // The seed reaches the second candidate, whose writes reach the first.
        reaches[0].block_writes = Some(cube(30.0, 1.0));
        reaches[1].block_writes = Some(cube(30.0, 1.0));
        reaches[2].block_writes = Some(cube(20.0, 1.0));
        reaches[3].block_writes = Some(cube(60.0, 1.0));
        let body = reaches[3].body;
        let reads = reaches[3].block_reads;
        prune_block_writes(
            &world,
            &mut reaches,
            &[
                BlockWriteCheck::Contact,
                BlockWriteCheck::Contact,
                BlockWriteCheck::Unconditional,
                BlockWriteCheck::Contact,
            ],
        )
        .ok_or("unexpected pruning budget exhaustion")?;
        assert!(
            reaches[..3]
                .iter()
                .all(|reach| reach.block_writes.is_some())
        );
        assert!(reaches[3].block_writes.is_none());
        assert_box(&reaches[3].body, &body);
        assert_box(&reaches[3].block_reads, &reads);
        world.level.shutdown().await;
        Ok(())
    }

    #[tokio::test]
    async fn final_contact_region_keeps_hazards_and_missing_chunks_conservative()
    -> Result<(), Box<dyn std::error::Error>> {
        let folder = tempfile::tempdir()?;
        let world = crate::entity::ai::test_support::world(&folder);
        let chunk = pumpkin_world::chunk::ChunkData::empty_sync(0, 0);
        world
            .level
            .loaded_chunks
            .insert(Vector2::new(0, 0), chunk.clone());
        let mut reach = node(0, 10.0, 0.0).1;
        reach.body = BoundingBox::new(Vector3::new(10.0, 64.0, 1.0), Vector3::new(18.0, 65.0, 2.0));
        reach.block_writes = Some(reach.body.expand(
            BLOCK_UPDATE_REACH,
            BLOCK_UPDATE_REACH,
            BLOCK_UPDATE_REACH,
        ));
        let original = reach.clone();
        prune_block_writes(
            &world,
            std::slice::from_mut(&mut reach),
            &[BlockWriteCheck::Contact],
        )
        .ok_or("unexpected pruning budget exhaustion")?;
        assert!(reach.block_writes.is_some());
        let neighbor = pumpkin_world::chunk::ChunkData::empty_sync(1, 0);
        world
            .level
            .loaded_chunks
            .insert(Vector2::new(1, 0), neighbor.clone());
        neighbor.set_block_absolute_y(1, 63, 1, Block::STONE_PRESSURE_PLATE.default_state.id);
        prune_block_writes(
            &world,
            std::slice::from_mut(&mut reach),
            &[BlockWriteCheck::Contact],
        )
        .ok_or("unexpected pruning budget exhaustion")?;
        assert!(reach.block_writes.is_some());
        neighbor.set_block_absolute_y(1, 63, 1, Block::AIR.default_state.id);
        prune_block_writes(
            &world,
            std::slice::from_mut(&mut reach),
            &[BlockWriteCheck::Contact],
        )
        .ok_or("unexpected pruning budget exhaustion")?;
        assert!(reach.block_writes.is_none());
        assert_box(&reach.body, &original.body);
        world.level.shutdown().await;
        Ok(())
    }

    #[tokio::test]
    async fn hurt_reactions_join_what_they_reach_with_the_hitter()
    -> Result<(), Box<dyn std::error::Error>> {
        let folder = tempfile::tempdir()?;
        let world = crate::entity::ai::test_support::world(&folder);
        let config = EntityIslandsConfig::default();
        // The hitter reaches the victim's body, the reaction reaches the far node.
        let hitter = node(1, 0.0, 2.0).1;
        let far = node(2, 30.0, 0.0).1;
        let victim = cube(2.5, 1.0);
        let hurt = HurtReach {
            area: victim.expand(40.0, 40.0, 40.0),
            changes_blocks: false,
        };
        let planned = |ticking: &[(i32, TickReach)],
                       stand_ins: &[(i32, TickReach)],
                       idle: &[IdleEntity]|
         -> Result<Vec<Vec<usize>>, &'static str> {
            let ids: Vec<_> = ticking.iter().map(|(id, _)| *id).collect();
            let reports = ticking
                .iter()
                .map(|(_, reach)| EntityReach::Conservative(reach.clone()))
                .collect();
            let checks = vec![BlockWriteCheck::Unconditional; ids.len()];
            let mut islands = plan(
                &world,
                &ids,
                reports,
                &checks,
                stand_ins,
                idle,
                &[],
                &config,
            )
            .ok_or("unexpected serial fallback")?
            .islands;
            islands.retain(|island| !island.is_empty());
            islands.sort();
            Ok(islands)
        };
        let nodes = [(1, hitter.clone()), (2, far.clone())];
        assert_eq!(
            planned(&nodes, &[], &[(3, victim, &EntityType::ENDERMAN)])?,
            [vec![0], vec![1]]
        );
        let idle_stand_in = [(3, hurt.stand_in(victim, &EntityType::ENDERMAN, Vec::new()))];
        assert_eq!(planned(&nodes, &idle_stand_in, &[])?, [vec![0, 1]]);

        let mut ticking_victim = node(3, 2.5, 0.0).1;
        ticking_victim.body = victim;
        ticking_victim.entities = victim;
        let nodes = [(1, hitter), (2, far), (3, ticking_victim)];
        assert_eq!(planned(&nodes, &[], &[])?, [vec![0, 2], vec![1]]);
        let linked_stand_in = [(!3, hurt.stand_in(victim, &EntityType::ENDERMAN, vec![3]))];
        assert_eq!(planned(&nodes, &linked_stand_in, &[])?, [vec![0, 1, 2]]);
        world.level.shutdown().await;
        Ok(())
    }

    #[tokio::test]
    async fn chest_cart_writes_only_bridge_elevated_readers_next_to_a_contact_block()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::entity::{Entity, vehicle::minecart::MinecartEntity};

        let folder = tempfile::tempdir()?;
        let world = crate::entity::ai::test_support::world(&folder);
        let chunk = pumpkin_world::chunk::ChunkData::empty_sync(0, 0);
        world
            .level
            .loaded_chunks
            .insert(Vector2::new(0, 0), chunk.clone());
        let cart = MinecartEntity::new(Entity::new(
            world.clone(),
            Vector3::new(8.5, 40.0, 8.5),
            &EntityType::CHEST_MINECART,
        ));
        let config = EntityIslandsConfig::default();
        let custom = Entity::new(
            world.clone(),
            cart.get_entity().pos.load(),
            &EntityType::CHEST_MINECART,
        );
        assert!(matches!(
            EntityReach::of(&custom, &config).block_write_check(),
            BlockWriteCheck::Unconditional
        ));
        let mut reader = node(1, 8.0, 0.0).1;
        reader.body =
            BoundingBox::new(Vector3::new(8.0, 200.0, 8.0), Vector3::new(9.0, 201.0, 9.0));
        reader.entities = reader.body;
        reader.players = reader.body;
        reader.block_reads = reader.body;
        reader.block_reads.min.y = f64::NEG_INFINITY;
        reader.block_reads.max.y = f64::INFINITY;
        let partitioned = || -> Result<Vec<Vec<usize>>, &'static str> {
            let report = EntityReach::of(&cart, &config);
            assert!(matches!(report, EntityReach::Audited(_)));
            let checks = [report.block_write_check(), BlockWriteCheck::Unconditional];
            let mut reaches = resolve(vec![report], &config).ok_or("unexpected serial fallback")?;
            reaches.push(reader.clone());
            prune_block_writes(&world, &mut reaches, &checks)
                .ok_or("unexpected pruning budget exhaustion")?;
            let nodes: Vec<_> = reaches
                .into_iter()
                .enumerate()
                .map(|(id, reach)| (id as i32, reach))
                .collect();
            Ok(islands(&nodes, &[], &[]))
        };
        assert_eq!(partitioned()?, [vec![0], vec![1]]);

        chunk.set_block_absolute_y(8, 39, 8, Block::DETECTOR_RAIL.default_state.id);
        assert_eq!(partitioned()?, [vec![0, 1]]);
        world.level.shutdown().await;
        Ok(())
    }
}
