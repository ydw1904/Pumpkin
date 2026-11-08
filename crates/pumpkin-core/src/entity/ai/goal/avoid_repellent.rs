use super::{Controls, Goal};
use crate::entity::ai::goal::revenge::MobFilter;
use crate::entity::ai::util::land_random_pos;
use crate::entity::{ai::pathfinder::NavigatorGoal, mob::Mob};
use crate::world::World;
use pumpkin_data::{Block, BlockState};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

type RepellentProvider = fn(&dyn Mob) -> Option<BlockPos>;

/// Vanilla `SetWalkTargetAwayFrom.pos` for the `NEAREST_REPELLENT` memory.
const DESIRED_DISTANCE: f64 = 8.0;
const SPEED: f64 = 1.0;
const FLEE_HORIZONTAL_DISTANCE: i32 = 16;
const FLEE_VERTICAL_DISTANCE: i32 = 7;
const FLEE_ATTEMPTS: usize = 10;

/// Vanilla `level.findBlocksInBoxByManhattanDistance(center, horizontal, vertical)` filtered and
/// `findFirst`: the matching block nearest to `center` by Manhattan distance.
pub fn find_nearest_block_by_manhattan_distance(
    world: &World,
    center: BlockPos,
    horizontal: i32,
    vertical: i32,
    matches: impl Fn(&Block, &BlockState) -> bool,
) -> Option<BlockPos> {
    let mut nearest: Option<(i32, BlockPos)> = None;
    for dy in -vertical..=vertical {
        for dx in -horizontal..=horizontal {
            for dz in -horizontal..=horizontal {
                let distance = dx.abs() + dy.abs() + dz.abs();
                if nearest.is_some_and(|(best, _)| best <= distance) {
                    continue;
                }
                let pos = BlockPos::new(center.0.x + dx, center.0.y + dy, center.0.z + dz);
                let (block, state) = world.get_block_and_state(&pos);
                if matches(block, state) {
                    nearest = Some((distance, pos));
                }
            }
        }
    }
    nearest.map(|(_, pos)| pos)
}

/// Walks away from the nearest repellent block once it is within 8 blocks.
pub struct AvoidRepellentGoal {
    repellent: RepellentProvider,
    /// Vanilla `interruptCurrentWalk`: start even while the mob walks elsewhere.
    interrupt_current_walk: bool,
    gate: Option<MobFilter>,
    target: Option<Vector3<f64>>,
}

impl AvoidRepellentGoal {
    #[must_use]
    pub const fn new(repellent: RepellentProvider, interrupt_current_walk: bool) -> Self {
        Self {
            repellent,
            interrupt_current_walk,
            gate: None,
            target: None,
        }
    }

    /// Only starts while `gate` holds, like a behaviour of a single brain activity.
    #[must_use]
    pub const fn with_gate(mut self, gate: MobFilter) -> Self {
        self.gate = Some(gate);
        self
    }
}

impl Goal for AvoidRepellentGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        if self.gate.is_some_and(|gate| !gate(mob)) {
            return false;
        }
        if !self.interrupt_current_walk && !mob.is_navigator_idle() {
            return false;
        }
        let Some(repellent) = (self.repellent)(mob) else {
            return false;
        };
        let avoid = repellent.to_f64().add_raw(0.5, 0.0, 0.5);
        let pos = mob.get_entity().pos.load();
        if pos.squared_distance_to_vec(&avoid) >= DESIRED_DISTANCE * DESIRED_DISTANCE {
            return false;
        }
        self.target = (0..FLEE_ATTEMPTS).find_map(|_| {
            land_random_pos::get_pos_away(
                mob,
                FLEE_HORIZONTAL_DISTANCE,
                FLEE_VERTICAL_DISTANCE,
                avoid,
            )
        });
        self.target.is_some()
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        !mob.is_navigator_idle()
    }

    fn start(&mut self, mob: &dyn Mob) {
        if let Some(target) = self.target {
            let pos = mob.get_entity().pos.load();
            mob.get_mob_entity()
                .steered()
                .navigator()
                .set_progress(NavigatorGoal::new(pos, target, SPEED));
        }
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.target = None;
        mob.get_mob_entity().steered().navigator().stop();
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}
