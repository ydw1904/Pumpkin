use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

use super::{Controls, Goal, GoalReach};
use crate::block::blocks::doors::DoorBlock;
use crate::entity::ai::pathfinder::path::Path;
use crate::entity::ai::util::{default_random_pos, land_random_pos};
use crate::entity::mob::Mob;
use pumpkin_data::tag::{PointOfInterestType, Taggable};
use pumpkin_world::poi::Occupancy;

/// How many visited POIs vanilla remembers before forgetting the oldest.
const MAX_VISITED: usize = 15;
/// The radius vanilla searches for an occupied village POI around a candidate.
const POI_SEARCH_RADIUS: i32 = 10;
/// Vanilla only starts within this many sections of a village.
const VILLAGE_SECTION_RANGE: i32 = 6;

/// Vanilla `MoveThroughVillageGoal`: at night, zombies walk from one occupied village POI to the
/// next.
pub struct MoveThroughVillageGoal {
    speed: f64,
    path: Option<Path>,
    poi_pos: BlockPos,
    only_at_night: bool,
    visited: Vec<BlockPos>,
    distance_to_poi: i32,
    can_deal_with_doors: Box<dyn Fn() -> bool + Send + Sync>,
}

impl MoveThroughVillageGoal {
    #[must_use]
    pub fn new(
        speed: f64,
        only_at_night: bool,
        distance_to_poi: i32,
        can_deal_with_doors: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Self {
        Self {
            speed,
            path: None,
            poi_pos: BlockPos::ZERO,
            only_at_night,
            visited: Vec::new(),
            distance_to_poi,
            can_deal_with_doors: Box::new(can_deal_with_doors),
        }
    }

    fn has_not_visited(&self, poi: BlockPos) -> bool {
        !self.visited.contains(&poi)
    }

    fn update_visited(&mut self) {
        if self.visited.len() > MAX_VISITED {
            self.visited.remove(0);
        }
    }

    /// Vanilla `createPath` with the door setting `canDealWithDoors` gives, restored to `true`
    /// afterwards. That restore is a vanilla quirk: it leaves the mob able to path through doors
    /// for good.
    fn create_path_with_doors(&self, mob: &dyn Mob, target: BlockPos) -> Option<Path> {
        let living = &mob.get_mob_entity().living_entity;
        let steered = mob.get_mob_entity().steered();
        let mut navigator = steered.navigator();
        navigator.set_can_open_doors((self.can_deal_with_doors)());
        let path = navigator.create_path(living, target.0.to_f64(), 0);
        navigator.set_can_open_doors(true);
        path
    }

    fn closer_to_poi_than(&self, mob: &dyn Mob, distance: f64) -> bool {
        self.poi_pos
            .to_centered_f64()
            .squared_distance_to_vec(&mob.get_entity().pos.load())
            < distance * distance
    }
}

impl Goal for MoveThroughVillageGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        if !mob
            .get_mob_entity()
            .steered()
            .navigator()
            .can_navigate_ground()
        {
            return false;
        }
        self.update_visited();
        let world = mob.get_entity().world.load_full();
        if self.only_at_night && world.is_bright_outside() {
            return false;
        }
        let pos = mob.get_entity().block_pos.load();
        if !world.is_close_to_village(&pos, VILLAGE_SECTION_RANGE) {
            return false;
        }

        let find_village_poi = |center: BlockPos| {
            world
                .portal_poi
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .find_closest(
                    &|poi_type| poi_type.has_tag(&PointOfInterestType::MINECRAFT_VILLAGE),
                    &|poi| self.has_not_visited(poi),
                    center,
                    POI_SEARCH_RADIUS,
                    Occupancy::IsOccupied,
                )
        };
        let Some(land_pos) = land_random_pos::get_pos_weighted(mob, 15, 7, |candidate| {
            if !world.is_village(candidate) {
                return f64::NEG_INFINITY;
            }
            find_village_poi(*candidate).map_or(f64::NEG_INFINITY, |poi| {
                let delta = poi.0 - pos.0;
                -f64::from(delta.x * delta.x + delta.y * delta.y + delta.z * delta.z)
            })
        }) else {
            return false;
        };
        let Some(target) = find_village_poi(BlockPos::floored_v(land_pos)) else {
            return false;
        };
        self.poi_pos = target;

        let mut path = self.create_path_with_doors(mob, self.poi_pos);
        if path.is_none() {
            let bottom_center = Vector3::new(
                f64::from(self.poi_pos.0.x) + 0.5,
                f64::from(self.poi_pos.0.y),
                f64::from(self.poi_pos.0.z) + 0.5,
            );
            let Some(partial_step) = default_random_pos::get_pos_towards(
                mob,
                10,
                7,
                bottom_center,
                std::f64::consts::FRAC_PI_2,
            ) else {
                return false;
            };
            path = self.create_path_with_doors(mob, BlockPos::floored_v(partial_step));
        }
        let Some(found) = path else {
            return false;
        };

        // Stop short of the first wooden door on the way.
        let door_node = (0..found.get_node_count())
            .filter_map(|index| found.get_node(index))
            .map(|node| node.pos)
            .find(|node| DoorBlock::is_wooden_door(&world, &node.up()));
        self.path = door_node.map_or(Some(found), |node| {
            let living = &mob.get_mob_entity().living_entity;
            let steered = mob.get_mob_entity().steered();
            let mut navigator = steered.navigator();
            navigator.create_path(living, node.0.to_f64(), 0)
        });
        self.path.is_some()
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        if mob.get_mob_entity().steered().navigator().is_done() {
            return false;
        }
        let width = f64::from(mob.get_entity().width());
        !self.closer_to_poi_than(mob, width + f64::from(self.distance_to_poi))
    }

    fn start(&mut self, mob: &dyn Mob) {
        let living = &mob.get_mob_entity().living_entity;
        let steered = mob.get_mob_entity().steered();
        steered
            .navigator()
            .move_to_path(self.path.take(), self.speed, living);
    }

    fn stop(&mut self, mob: &dyn Mob) {
        if mob.get_mob_entity().steered().navigator().is_done()
            || self.closer_to_poi_than(mob, f64::from(self.distance_to_poi))
        {
            self.visited.push(self.poi_pos);
        }
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }

    fn tick_reach(&self, mob: &dyn Mob, _running: bool) -> Option<GoalReach> {
        // Starting reads village POI tickets, which villagers anywhere nearby change. By day a
        // night-only goal cannot start, and a running one only follows its route.
        (self.only_at_night && mob.get_entity().world.load().is_bright_outside()).then(|| {
            GoalReach {
                speed: self.speed,
                ..GoalReach::default()
            }
        })
    }
}
