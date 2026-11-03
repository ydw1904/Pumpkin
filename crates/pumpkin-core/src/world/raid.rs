use pumpkin_util::random::gameplay::game_random;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use uuid::Uuid;

use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::entity::EntityType;
use pumpkin_data::potion::Effect;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_util::difficulty::Difficulty;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector2::Vector2;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::text::TextComponent;
use pumpkin_world::chunk::ChunkHeightmapType;

use crate::entity::EntityBase;
use crate::entity::mob::raider::{Raider, create_ominous_banner};
use crate::entity::mob::spawn::finalize_spawn;
use crate::entity::player::Player;
use crate::entity::r#type::from_type;
use crate::world::World;
use crate::world::bossbar::{Bossbar, BossbarColor, BossbarDivisions};
use crate::world::natural_spawner::is_spawn_position_ok;
use crate::world::villager_poi::Occupancy;

const RAID_NAME: &str = "event.minecraft.raid";
const RAIDERS_REMAINING: &str = "event.minecraft.raid.raiders_remaining";
const RAID_BAR_VICTORY: &str = "event.minecraft.raid.victory.full";
const RAID_BAR_DEFEAT: &str = "event.minecraft.raid.defeat.full";
const ALLOW_SPAWNING_WITHIN_VILLAGE_SECONDS_THRESHOLD: i32 = 7;
const SECTION_RADIUS_FOR_FINDING_NEW_VILLAGE_CENTER: i32 = 2;
const RAID_TIMEOUT_TICKS: i64 = 48000;
const NUM_SPAWN_ATTEMPTS: i32 = 5;
const POST_RAID_TICK_LIMIT: i32 = 40;
const DEFAULT_PRE_RAID_TICKS: i32 = 300;
const MAX_NO_ACTION_TIME: i32 = 2400;
const MAX_CELEBRATION_TICKS: i32 = 600;
const OUTSIDE_RAID_BOUNDS_TIMEOUT: i32 = 30;
const LOW_MOB_THRESHOLD: usize = 2;
const HERO_OF_THE_VILLAGE_DURATION: i32 = 48000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RaidStatus {
    Ongoing,
    Victory,
    Loss,
    Stopped,
}

impl RaidStatus {
    #[must_use]
    pub const fn get_serialized_name(self) -> &'static str {
        match self {
            Self::Ongoing => "ongoing",
            Self::Victory => "victory",
            Self::Loss => "loss",
            Self::Stopped => "stopped",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RaiderType {
    Vindicator,
    Evoker,
    Pillager,
    Witch,
    Ravager,
}

impl RaiderType {
    pub const VALUES: [Self; 5] = [
        Self::Vindicator,
        Self::Evoker,
        Self::Pillager,
        Self::Witch,
        Self::Ravager,
    ];

    #[must_use]
    pub const fn entity_type(self) -> &'static EntityType {
        match self {
            Self::Vindicator => &EntityType::VINDICATOR,
            Self::Evoker => &EntityType::EVOKER,
            Self::Pillager => &EntityType::PILLAGER,
            Self::Witch => &EntityType::WITCH,
            Self::Ravager => &EntityType::RAVAGER,
        }
    }

    #[must_use]
    pub const fn spawns_per_wave(self) -> &'static [i32; 8] {
        match self {
            Self::Vindicator => &[0, 0, 2, 0, 1, 4, 2, 5],
            Self::Evoker => &[0, 0, 0, 0, 0, 1, 1, 2],
            Self::Pillager => &[0, 4, 3, 3, 4, 4, 4, 2],
            Self::Witch => &[0, 0, 0, 0, 3, 0, 0, 1],
            Self::Ravager => &[0, 0, 0, 1, 0, 1, 0, 2],
        }
    }
}

/// Vanilla `Raid`. Raiders are kept by UUID.
pub struct Raid {
    pub id: i32,
    pub center: BlockPos,
    pub status: RaidStatus,
    pub active: bool,
    pub started: bool,
    pub ticks_active: i64,
    pub raid_omen_level: i32,
    pub groups_spawned: i32,
    pub num_groups: i32,
    pub raid_cooldown_ticks: i32,
    pub post_raid_ticks: i32,
    pub celebration_ticks: i32,
    pub total_health: f32,
    pub bossbar: Bossbar,
    bossbar_visible: bool,
    /// Set when a raider left the raid outside the raid tick; the bar is updated next tick.
    bossbar_dirty: bool,
    pub group_to_leader_map: HashMap<i32, Uuid>,
    pub group_raider_map: HashMap<i32, HashSet<Uuid>>,
    pub heroes_of_the_village: HashSet<Uuid>,
    pub players_in_raid: HashSet<Uuid>,
    pub wave_spawn_pos: Option<BlockPos>,
}

impl Raid {
    pub const VALID_RAID_RADIUS: f64 = 96.0;
    pub const VALID_RAID_RADIUS_SQR: f64 = 9216.0;
    pub const RAID_REMOVAL_THRESHOLD_SQR: f64 = 12544.0;
    pub const MAX_RAID_OMEN_LEVEL: i32 = 5;

    #[must_use]
    pub fn new(id: i32, center: BlockPos, difficulty: Difficulty) -> Self {
        let mut bossbar = Bossbar::new(TextComponent::translate(RAID_NAME, []));
        bossbar.color = BossbarColor::Red;
        bossbar.division = BossbarDivisions::Notches10;
        bossbar.health = 0.0;
        Self {
            id,
            center,
            status: RaidStatus::Ongoing,
            active: true,
            started: false,
            ticks_active: 0,
            raid_omen_level: 0,
            groups_spawned: 0,
            num_groups: Self::get_num_groups(difficulty),
            raid_cooldown_ticks: DEFAULT_PRE_RAID_TICKS,
            post_raid_ticks: 0,
            celebration_ticks: 0,
            total_health: 0.0,
            bossbar,
            bossbar_visible: true,
            bossbar_dirty: false,
            group_to_leader_map: HashMap::new(),
            group_raider_map: HashMap::new(),
            heroes_of_the_village: HashSet::new(),
            players_in_raid: HashSet::new(),
            wave_spawn_pos: None,
        }
    }

    #[must_use]
    pub const fn get_num_groups(difficulty: Difficulty) -> i32 {
        match difficulty {
            Difficulty::Peaceful => 0,
            Difficulty::Easy => 3,
            Difficulty::Normal => 5,
            Difficulty::Hard => 7,
        }
    }

    #[must_use]
    pub const fn is_over(&self) -> bool {
        self.is_victory() || self.is_loss()
    }

    #[must_use]
    pub const fn is_stopped(&self) -> bool {
        matches!(self.status, RaidStatus::Stopped)
    }

    #[must_use]
    pub const fn is_victory(&self) -> bool {
        matches!(self.status, RaidStatus::Victory)
    }

    #[must_use]
    pub const fn is_loss(&self) -> bool {
        matches!(self.status, RaidStatus::Loss)
    }

    #[must_use]
    pub const fn is_started(&self) -> bool {
        self.started
    }

    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.active
    }

    #[must_use]
    pub const fn get_groups_spawned(&self) -> i32 {
        self.groups_spawned
    }

    #[must_use]
    pub const fn get_raid_omen_level(&self) -> i32 {
        self.raid_omen_level
    }

    pub const fn set_raid_omen_level(&mut self, level: i32) {
        self.raid_omen_level = level;
    }

    #[must_use]
    pub const fn has_first_wave_spawned(&self) -> bool {
        self.groups_spawned > 0
    }

    const fn has_more_waves(&self) -> bool {
        if self.has_bonus_wave() {
            !self.has_spawned_bonus_wave()
        } else {
            !self.is_final_wave()
        }
    }

    const fn is_final_wave(&self) -> bool {
        self.groups_spawned == self.num_groups
    }

    const fn has_bonus_wave(&self) -> bool {
        self.raid_omen_level > 1
    }

    const fn has_spawned_bonus_wave(&self) -> bool {
        self.groups_spawned > self.num_groups
    }

    fn should_spawn_bonus_group(&self) -> bool {
        self.is_final_wave() && self.get_total_raiders_alive() == 0 && self.has_bonus_wave()
    }

    fn should_spawn_group(&self) -> bool {
        self.raid_cooldown_ticks == 0
            && (self.groups_spawned < self.num_groups || self.should_spawn_bonus_group())
            && self.get_total_raiders_alive() == 0
    }

    #[must_use]
    pub fn get_total_raiders_alive(&self) -> usize {
        self.group_raider_map.values().map(HashSet::len).sum()
    }

    #[must_use]
    pub fn get_all_raiders(&self) -> HashSet<Uuid> {
        self.group_raider_map.values().flatten().copied().collect()
    }

    pub fn stop(&mut self, world: &World) {
        self.active = false;
        self.remove_all_players(world);
        self.status = RaidStatus::Stopped;
    }

    pub fn remove_all_players(&mut self, world: &World) {
        for player in world.players.load().iter() {
            if self.players_in_raid.contains(&player.gameprofile.id) {
                player.remove_bossbar(self.bossbar.uuid);
            }
        }
        self.players_in_raid.clear();
    }

    /// Vanilla `Raid.absorbRaidOmen`: the player's Raid Omen raises the omen level.
    pub fn absorb_raid_omen(&mut self, player: &Player) -> bool {
        let Some(effect) = player.get_effect(&StatusEffect::RAID_OMEN) else {
            return false;
        };
        self.raid_omen_level = (self.raid_omen_level + i32::from(effect.amplifier) + 1)
            .clamp(0, Self::MAX_RAID_OMEN_LEVEL);
        if !self.has_first_wave_spawned() {
            player.increment_stat(
                pumpkin_data::statistic::StatisticCategory::Custom,
                pumpkin_data::statistic::CustomStatistic::RaidTrigger as i32,
                1,
            );
        }
        true
    }

    pub fn add_hero_of_the_village(&mut self, killer_uuid: Uuid) {
        self.heroes_of_the_village.insert(killer_uuid);
    }

    pub fn remove_leader(&mut self, wave: i32) {
        self.group_to_leader_map.remove(&wave);
    }

    /// Vanilla `Raid.removeFromRaid`; `health` is taken off the total when given.
    fn remove_from_raid(
        &mut self,
        wave: i32,
        raider_uuid: Uuid,
        raider: Option<&dyn Raider>,
        health: Option<f32>,
    ) {
        let removed = self
            .group_raider_map
            .get_mut(&wave)
            .is_some_and(|raiders| raiders.remove(&raider_uuid));
        if removed {
            if let Some(health) = health {
                self.total_health -= health;
            }
            if let Some(raider) = raider {
                raider.get_raider_data().raid_id.store(None);
            }
            self.bossbar_dirty = true;
        }
    }

    /// The raid part of vanilla `Raider.die`: a player who killed the raider becomes a hero.
    fn on_raider_death(&mut self, raider: &dyn Raider, uuid: Uuid, killed_by_player: Option<Uuid>) {
        if raider.is_patrol_leader() {
            self.remove_leader(raider.get_wave());
        }
        if let Some(player) = killed_by_player {
            self.add_hero_of_the_village(player);
        }
        self.remove_from_raid(raider.get_wave(), uuid, Some(raider), None);
    }

    fn get_default_num_spawns(
        &self,
        raider_type: RaiderType,
        wave: i32,
        is_bonus_wave: bool,
    ) -> i32 {
        let index = if is_bonus_wave { self.num_groups } else { wave };
        raider_type
            .spawns_per_wave()
            .get(index as usize)
            .copied()
            .unwrap_or(0)
    }

    fn get_potential_bonus_spawns(
        raider_type: RaiderType,
        wave: i32,
        difficulty: Difficulty,
        is_bonus_wave: bool,
    ) -> i32 {
        let is_easy = difficulty == Difficulty::Easy;
        let is_normal = difficulty == Difficulty::Normal;
        let bonus_spawns = match raider_type {
            RaiderType::Vindicator | RaiderType::Pillager => {
                if is_easy {
                    game_random().random_range(0..2)
                } else if is_normal {
                    1
                } else {
                    2
                }
            }
            RaiderType::Evoker => return 0,
            RaiderType::Witch => {
                if is_easy || wave <= 2 || wave == 4 {
                    return 0;
                }
                1
            }
            RaiderType::Ravager => i32::from(!is_easy && is_bonus_wave),
        };
        if bonus_spawns > 0 {
            game_random().random_range(0..=bonus_spawns)
        } else {
            0
        }
    }

    /// Vanilla `Raid.spawnGroup`.
    fn spawn_group(&mut self, world: &Arc<World>, pos: BlockPos) {
        let mut leader_set = false;
        let group_number = self.groups_spawned + 1;
        self.total_health = 0.0;
        let difficulty = world.level_info.load().difficulty;
        let is_bonus_group = self.should_spawn_bonus_group();

        for raider_type in RaiderType::VALUES {
            let num_spawns = self.get_default_num_spawns(raider_type, group_number, is_bonus_group)
                + Self::get_potential_bonus_spawns(
                    raider_type,
                    group_number,
                    difficulty,
                    is_bonus_group,
                );
            let mut ravagers_spawned = 0;

            for _ in 0..num_spawns {
                let raider_base = from_type(
                    raider_type.entity_type(),
                    pos.to_f64(),
                    world,
                    Uuid::new_v4(),
                );
                let Some(raider) = raider_base.get_mob().and_then(|mob| mob.as_raider()) else {
                    break;
                };
                if !leader_set && raider.can_be_leader() {
                    raider.set_patrol_leader(true);
                    self.set_leader(group_number, raider_base.as_ref());
                    leader_set = true;
                }
                self.join_raid(world, group_number, &raider_base, pos);

                if raider_type == RaiderType::Ravager {
                    let riding_type = if group_number == Self::get_num_groups(Difficulty::Normal) {
                        Some(&EntityType::PILLAGER)
                    } else if group_number >= Self::get_num_groups(Difficulty::Hard) {
                        if ravagers_spawned == 0 {
                            Some(&EntityType::EVOKER)
                        } else {
                            Some(&EntityType::VINDICATOR)
                        }
                    } else {
                        None
                    };
                    ravagers_spawned += 1;
                    if let Some(riding_type) = riding_type {
                        let rider = from_type(riding_type, pos.to_f64(), world, Uuid::new_v4());
                        self.join_raid(world, group_number, &rider, pos);
                        // Vanilla `snapTo(pos)` before `startRiding`.
                        rider.get_entity().set_pos(Vector3::new(
                            f64::from(pos.0.x) + 0.5,
                            f64::from(pos.0.y),
                            f64::from(pos.0.z) + 0.5,
                        ));
                        raider_base
                            .get_entity()
                            .add_passenger(raider_base.clone(), rider);
                    }
                }
            }
        }

        self.wave_spawn_pos = None;
        self.groups_spawned += 1;
        self.update_bossbar(world);
    }

    /// Vanilla `Raid.joinRaid` for a new raider, which is placed at `pos` and spawned.
    fn join_raid(
        &mut self,
        world: &Arc<World>,
        wave: i32,
        raider_base: &Arc<dyn EntityBase>,
        pos: BlockPos,
    ) {
        let Some(mob) = raider_base.get_mob() else {
            return;
        };
        let Some(raider) = mob.as_raider() else {
            return;
        };
        let entity = raider_base.get_entity();
        self.group_raider_map
            .entry(wave)
            .or_default()
            .insert(entity.entity_uuid);
        self.total_health += mob.get_mob_entity().living_entity.health.load();

        let data = raider.get_raider_data();
        data.raid_id.store(Some(self.id));
        raider.set_wave(wave);
        raider.set_can_join_raid(true);
        data.ticks_outside_raid
            .store(0, std::sync::atomic::Ordering::Relaxed);
        entity.set_pos(Vector3::new(
            f64::from(pos.0.x) + 0.5,
            f64::from(pos.0.y) + 1.0,
            f64::from(pos.0.z) + 0.5,
        ));
        finalize_spawn(raider_base, world, None);
        raider.apply_raid_buffs(wave, false);
        entity
            .on_ground
            .store(true, std::sync::atomic::Ordering::Relaxed);
        world.spawn_entity(raider_base.clone());
    }

    /// Vanilla `Raid.setLeader`: the leader wears the ominous banner and always drops it.
    fn set_leader(&mut self, wave: i32, raider_base: &dyn EntityBase) {
        let Some(mob) = raider_base.get_mob() else {
            return;
        };
        self.group_to_leader_map
            .insert(wave, raider_base.get_entity().entity_uuid);
        let mob_entity = mob.get_mob_entity();
        mob_entity.set_item_slot(&EquipmentSlot::HEAD, create_ominous_banner());
        mob_entity.set_guaranteed_drop(&EquipmentSlot::HEAD);
    }

    /// Vanilla `Raid.updateBossbar`.
    pub fn update_bossbar(&mut self, world: &World) {
        let progress = self.get_health_of_living_raiders(world) / self.total_health;
        let progress = if progress.is_nan() {
            0.0
        } else {
            progress.clamp(0.0, 1.0)
        };
        self.set_progress(world, progress);
    }

    #[must_use]
    pub fn get_health_of_living_raiders(&self, world: &World) -> f32 {
        self.group_raider_map
            .values()
            .flatten()
            .filter_map(|uuid| world.get_entity_by_uuid(*uuid))
            .filter_map(|entity| {
                entity
                    .get_living_entity()
                    .map(|living| living.health.load())
            })
            .sum()
    }

    /// `ServerBossEvent.setProgress`: only a change reaches the players.
    fn set_progress(&mut self, world: &World, progress: f32) {
        #[expect(
            clippy::float_cmp,
            reason = "vanilla sends the progress when it changed at all"
        )]
        if self.bossbar.health == progress {
            return;
        }
        self.bossbar.health = progress;
        self.for_each_bar_player(world, |player, bar| {
            player.update_bossbar_health(&bar.uuid, bar.health);
        });
    }

    /// `ServerBossEvent.setName`: only a change reaches the players.
    fn set_name(&mut self, world: &World, name: TextComponent) {
        if self.bossbar.title == name {
            return;
        }
        self.bossbar.title = name;
        self.for_each_bar_player(world, |player, bar| {
            player.update_bossbar_title(&bar.uuid, bar.title.clone());
        });
    }

    /// `ServerBossEvent.setVisible`.
    fn set_visible(&mut self, world: &World, visible: bool) {
        if self.bossbar_visible == visible {
            return;
        }
        self.bossbar_visible = visible;
        for player in world.players.load().iter() {
            if self.players_in_raid.contains(&player.gameprofile.id) {
                if visible {
                    player.send_bossbar(&self.bossbar);
                } else {
                    player.remove_bossbar(self.bossbar.uuid);
                }
            }
        }
    }

    fn for_each_bar_player(&self, world: &World, f: impl Fn(&Player, &Bossbar)) {
        if !self.bossbar_visible {
            return;
        }
        for player in world.players.load().iter() {
            if self.players_in_raid.contains(&player.gameprofile.id) {
                f(player, &self.bossbar);
            }
        }
    }

    /// Vanilla `Raid.updatePlayers`: the bar shows for living players whose nearest raid this
    /// is.
    fn update_players(&mut self, world: &World, raid_centers: &[(i32, BlockPos)]) {
        for player in world.players.load().iter() {
            let id = player.gameprofile.id;
            let pos = player.get_entity().block_pos.load();
            let valid = player.living_entity.health.load() > 0.0
                && nearest_raid(raid_centers, &pos) == Some(self.id);
            let was_in = self.players_in_raid.contains(&id);
            if valid && !was_in {
                self.players_in_raid.insert(id);
                if self.bossbar_visible {
                    player.send_bossbar(&self.bossbar);
                }
            } else if !valid && was_in {
                self.players_in_raid.remove(&id);
                if self.bossbar_visible {
                    player.remove_bossbar(self.bossbar.uuid);
                }
            }
        }
    }

    /// Vanilla `Raid.updateRaiders`: raiders that despawned, wandered off or idled outside the
    /// village leave the raid.
    fn update_raiders(&mut self, world: &World) {
        let mut to_remove = Vec::new();
        for (&wave, raiders) in &self.group_raider_map {
            for &uuid in raiders {
                let Some(entity) = world.get_entity_by_uuid(uuid) else {
                    to_remove.push((wave, uuid));
                    continue;
                };
                let base = entity.get_entity();
                let raider_pos = base.block_pos.load();
                if base.removed.load(std::sync::atomic::Ordering::Relaxed)
                    || block_dist_squared(self.center, raider_pos)
                        >= Self::RAID_REMOVAL_THRESHOLD_SQR
                {
                    to_remove.push((wave, uuid));
                } else if base.tick_count.load(std::sync::atomic::Ordering::Relaxed) > 600
                    && let Some(raider) = entity.get_mob().and_then(|mob| mob.as_raider())
                {
                    let outside = &raider.get_raider_data().ticks_outside_raid;
                    let no_action_time = raider
                        .get_mob_entity()
                        .living_entity
                        .no_action_time
                        .load(std::sync::atomic::Ordering::Relaxed);
                    if !world.is_village(&raider_pos) && no_action_time > MAX_NO_ACTION_TIME {
                        outside.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                    if outside.load(std::sync::atomic::Ordering::Relaxed)
                        >= OUTSIDE_RAID_BOUNDS_TIMEOUT
                    {
                        to_remove.push((wave, uuid));
                    }
                }
            }
        }

        for (wave, uuid) in to_remove {
            let entity = world.get_entity_by_uuid(uuid);
            let raider = entity
                .as_deref()
                .and_then(EntityBase::get_mob)
                .and_then(|mob| mob.as_raider());
            let health = raider.map_or(0.0, |raider| {
                raider.get_mob_entity().living_entity.health.load()
            });
            self.remove_from_raid(wave, uuid, raider, Some(health));
            if matches!(raider, Some(raider) if raider.is_patrol_leader()) {
                self.remove_leader(wave);
            }
        }
        if self.bossbar_dirty {
            self.bossbar_dirty = false;
            self.update_bossbar(world);
        }
    }

    /// Vanilla `Raid.playSound`: each player hears the horn from 13 blocks towards the raid.
    fn play_sound(&self, world: &World, sound_origin: BlockPos) {
        let seed = game_random().random::<i64>();
        let raid_loc = sound_origin.to_centered_f64();
        for player in world.players.load().iter() {
            let player_loc = player.get_entity().pos.load();
            let dist = (raid_loc.x - player_loc.x).hypot(raid_loc.z - player_loc.z);
            let x = player_loc.x + 13.0 / dist * (raid_loc.x - player_loc.x);
            let z = player_loc.z + 13.0 / dist * (raid_loc.z - player_loc.z);
            if dist <= 64.0 || self.players_in_raid.contains(&player.gameprofile.id) {
                player.play_sound(
                    Sound::EventRaidHorn as u16,
                    SoundCategory::Neutral,
                    &Vector3::new(x, player_loc.y, z),
                    64.0,
                    1.0,
                    seed,
                );
            }
        }
    }

    /// Vanilla `Raid.moveRaidCenterToNearbyVillageSection`.
    fn move_raid_center_to_nearby_village_section(&mut self, world: &World) {
        let r = SECTION_RADIUS_FOR_FINDING_NEW_VILLAGE_CENTER;
        let (sx, sy, sz) = (
            self.center.0.x >> 4,
            self.center.0.y >> 4,
            self.center.0.z >> 4,
        );
        let mut best: Option<(i64, BlockPos)> = None;
        for dx in -r..=r {
            for dy in -r..=r {
                for dz in -r..=r {
                    // `SectionPos.center`.
                    let center = BlockPos::new(
                        ((sx + dx) << 4) + 8,
                        ((sy + dy) << 4) + 8,
                        ((sz + dz) << 4) + 8,
                    );
                    if !world.is_village(&center) {
                        continue;
                    }
                    let dist = block_dist_squared_i64(center, self.center);
                    if best.is_none_or(|(best_dist, _)| dist < best_dist) {
                        best = Some((dist, center));
                    }
                }
            }
        }
        if let Some((_, center)) = best {
            self.center = center;
        }
    }

    /// Vanilla `Raid.findRandomSpawnPos`: further out the more of the countdown is left, and
    /// inside the village only in its last seconds.
    fn find_random_spawn_pos(&self, world: &Arc<World>, max_tries: i32) -> Option<BlockPos> {
        let seconds_remaining = self.raid_cooldown_ticks / 20;
        let how_far = 0.22 * seconds_remaining as f32 - 0.24;
        let rng = game_random();
        let start_angle = rng.random::<f32>() * std::f32::consts::TAU;
        for i in 0..max_tries {
            let angle = start_angle + std::f32::consts::PI * i as f32 / 8.0;
            let spawn_x = self.center.0.x
                + (angle.cos() * 32.0 * how_far).floor() as i32
                + rng.random_range(0..3) * how_far.floor() as i32;
            let spawn_z = self.center.0.z
                + (angle.sin() * 32.0 * how_far).floor() as i32
                + rng.random_range(0..3) * how_far.floor() as i32;
            // `getHeight(WORLD_SURFACE)` is the first block above the surface.
            let spawn_y =
                world.get_heightmap_height(ChunkHeightmapType::WorldSurface, spawn_x, spawn_z) + 1;
            if (spawn_y - self.center.0.y).abs() > 96 {
                continue;
            }
            let spawn_pos = BlockPos::new(spawn_x, spawn_y, spawn_z);
            if world.is_village(&spawn_pos)
                && seconds_remaining > ALLOW_SPAWNING_WITHIN_VILLAGE_SECONDS_THRESHOLD
            {
                continue;
            }
            if has_chunks_at(
                world,
                spawn_x - 10,
                spawn_z - 10,
                spawn_x + 10,
                spawn_z + 10,
            ) && world.is_position_entity_ticking(&spawn_pos)
                && (is_spawn_position_ok(world, &spawn_pos, &EntityType::RAVAGER)
                    || (world.get_block(&spawn_pos.down()) == &pumpkin_data::Block::SNOW
                        && world.get_block_state(&spawn_pos).is_air()))
            {
                return Some(spawn_pos);
            }
        }
        None
    }

    /// Vanilla `Raid.tick`. `raid_centers` are the active raids, for `getRaidAt`.
    #[allow(clippy::too_many_lines)]
    fn tick(&mut self, world: &Arc<World>, raid_centers: &[(i32, BlockPos)]) {
        if self.is_stopped() {
            return;
        }

        if self.status == RaidStatus::Ongoing {
            let old_active = self.active;
            self.active = world.level.is_chunk_loaded(&self.center.chunk_position());
            if world.level_info.load().difficulty == Difficulty::Peaceful {
                self.stop(world);
                return;
            }
            if old_active != self.active {
                self.set_visible(world, self.active);
            }
            if !self.active {
                return;
            }

            if !world.is_village(&self.center) {
                self.move_raid_center_to_nearby_village_section(world);
            }
            if !world.is_village(&self.center) {
                if self.groups_spawned > 0 {
                    self.status = RaidStatus::Loss;
                } else {
                    self.stop(world);
                }
            }

            self.ticks_active += 1;
            if self.ticks_active >= RAID_TIMEOUT_TICKS {
                self.stop(world);
                return;
            }

            let raiders_alive = self.get_total_raiders_alive();
            if raiders_alive == 0 && self.has_more_waves() {
                if self.raid_cooldown_ticks <= 0 {
                    if self.raid_cooldown_ticks == 0 && self.groups_spawned > 0 {
                        self.raid_cooldown_ticks = DEFAULT_PRE_RAID_TICKS;
                        self.set_name(world, TextComponent::translate(RAID_NAME, []));
                        return;
                    }
                } else {
                    let should_try = self
                        .wave_spawn_pos
                        .map_or(self.raid_cooldown_ticks % 5 == 0, |pos| {
                            !world.is_position_entity_ticking(&pos)
                        });
                    if should_try {
                        self.wave_spawn_pos = self.find_random_spawn_pos(world, 8);
                    }
                    if self.raid_cooldown_ticks == DEFAULT_PRE_RAID_TICKS
                        || self.raid_cooldown_ticks % 20 == 0
                    {
                        self.update_players(world, raid_centers);
                    }
                    self.raid_cooldown_ticks -= 1;
                    let progress = (DEFAULT_PRE_RAID_TICKS - self.raid_cooldown_ticks) as f32
                        / DEFAULT_PRE_RAID_TICKS as f32;
                    self.set_progress(world, progress.clamp(0.0, 1.0));
                }
            }

            if self.ticks_active % 20 == 0 {
                self.update_players(world, raid_centers);
                self.update_raiders(world);
                let name = if raiders_alive > 0 && raiders_alive <= LOW_MOB_THRESHOLD {
                    TextComponent::translate(RAID_NAME, [])
                        .add_text(" - ")
                        .add_child(TextComponent::translate(
                            RAIDERS_REMAINING,
                            [TextComponent::text(raiders_alive.to_string())],
                        ))
                } else {
                    TextComponent::translate(RAID_NAME, [])
                };
                self.set_name(world, name);
            } else if self.bossbar_dirty {
                self.bossbar_dirty = false;
                self.update_bossbar(world);
            }

            let mut sound_played = false;
            let mut attempt = 0;
            while self.should_spawn_group() {
                let spawn_pos = self
                    .wave_spawn_pos
                    .or_else(|| self.find_random_spawn_pos(world, 20));
                if let Some(spawn_pos) = spawn_pos {
                    self.started = true;
                    self.spawn_group(world, spawn_pos);
                    if !sound_played {
                        self.play_sound(world, spawn_pos);
                        sound_played = true;
                    }
                } else {
                    attempt += 1;
                }
                if attempt > NUM_SPAWN_ATTEMPTS {
                    self.stop(world);
                    break;
                }
            }

            if self.is_started() && !self.has_more_waves() && raiders_alive == 0 {
                if self.post_raid_ticks < POST_RAID_TICK_LIMIT {
                    self.post_raid_ticks += 1;
                } else {
                    self.status = RaidStatus::Victory;
                    self.reward_heroes(world);
                }
            }
        } else if self.is_over() {
            self.celebration_ticks += 1;
            if self.celebration_ticks >= MAX_CELEBRATION_TICKS {
                self.stop(world);
                return;
            }
            if self.celebration_ticks % 20 == 0 {
                self.update_players(world, raid_centers);
                self.set_visible(world, true);
                if self.is_victory() {
                    self.set_progress(world, 0.0);
                    self.set_name(world, TextComponent::translate(RAID_BAR_VICTORY, []));
                } else {
                    self.set_name(world, TextComponent::translate(RAID_BAR_DEFEAT, []));
                }
            }
        }
    }

    /// Vanilla `Raid.MAP_CODEC` with the raid's id.
    fn to_nbt(&self) -> NbtCompound {
        let mut nbt = NbtCompound::new();
        nbt.put_int("id", self.id);
        nbt.put_bool("started", self.started);
        nbt.put_bool("active", self.active);
        nbt.put_long("ticks_active", self.ticks_active);
        nbt.put_int("raid_omen_level", self.raid_omen_level);
        nbt.put_int("groups_spawned", self.groups_spawned);
        nbt.put_int("cooldown_ticks", self.raid_cooldown_ticks);
        nbt.put_int("post_raid_ticks", self.post_raid_ticks);
        nbt.put_float("total_health", self.total_health);
        nbt.put_int("group_count", self.num_groups);
        nbt.put_string("status", self.status.get_serialized_name().to_string());
        nbt.put(
            "center",
            NbtTag::IntArray(vec![self.center.0.x, self.center.0.y, self.center.0.z]),
        );
        let mut heroes: Vec<&Uuid> = self.heroes_of_the_village.iter().collect();
        heroes.sort_unstable();
        nbt.put_list(
            "heroes_of_the_village",
            heroes.into_iter().map(|uuid| NbtTag::uuid(*uuid)).collect(),
        );
        nbt
    }

    fn from_nbt(nbt: &NbtCompound) -> Option<Self> {
        let center = nbt.get_int_array("center")?;
        let [x, y, z] = *center else {
            return None;
        };
        let status = [
            RaidStatus::Ongoing,
            RaidStatus::Victory,
            RaidStatus::Loss,
            RaidStatus::Stopped,
        ]
        .into_iter()
        .find(|status| Some(status.get_serialized_name()) == nbt.get_string("status"))?;
        let mut raid = Self::new(
            nbt.get_int("id")?,
            BlockPos::new(x, y, z),
            Difficulty::Peaceful,
        );
        raid.started = nbt.get_bool("started")?;
        raid.active = nbt.get_bool("active")?;
        raid.ticks_active = nbt.get_long("ticks_active")?;
        raid.raid_omen_level = nbt.get_int("raid_omen_level")?;
        raid.groups_spawned = nbt.get_int("groups_spawned")?;
        raid.raid_cooldown_ticks = nbt.get_int("cooldown_ticks")?;
        raid.post_raid_ticks = nbt.get_int("post_raid_ticks")?;
        raid.total_health = nbt.get_float("total_health")?;
        raid.num_groups = nbt.get_int("group_count")?;
        raid.status = status;
        raid.heroes_of_the_village = nbt
            .get_list("heroes_of_the_village")?
            .iter()
            .filter_map(NbtTag::extract_uuid)
            .collect();
        // A loaded `ServerBossEvent` starts full, like any new boss bar.
        raid.bossbar.health = 1.0;
        Some(raid)
    }

    fn reward_heroes(&self, world: &World) {
        let effect = Effect {
            effect_type: &StatusEffect::HERO_OF_THE_VILLAGE,
            duration: HERO_OF_THE_VILLAGE_DURATION,
            // `MobEffectInstance` clamps the amplifier into 0..=255.
            amplifier: (self.raid_omen_level - 1).clamp(0, 255) as u8,
            ambient: false,
            show_particles: false,
            show_icon: true,
            blend: true,
        };
        for player in world.players.load().iter() {
            if self.heroes_of_the_village.contains(&player.gameprofile.id) && !player.is_spectator()
            {
                player.add_effect(effect.clone());
                player.increment_stat(
                    pumpkin_data::statistic::StatisticCategory::Custom,
                    pumpkin_data::statistic::CustomStatistic::RaidWin as i32,
                    1,
                );
                player.trigger_advancement(
                    crate::entity::player::advancement::trigger::AdvancementTrigger::HeroOfTheVillage,
                );
            }
        }
    }
}

/// Vanilla `Vec3i.distSqr` between two block positions.
fn block_dist_squared(a: BlockPos, b: BlockPos) -> f64 {
    block_dist_squared_i64(a, b) as f64
}

fn block_dist_squared_i64(a: BlockPos, b: BlockPos) -> i64 {
    let delta = a.0 - b.0;
    i64::from(delta.x).pow(2) + i64::from(delta.y).pow(2) + i64::from(delta.z).pow(2)
}

/// Vanilla `Level.hasChunksAt` for a block area.
fn has_chunks_at(world: &World, min_x: i32, min_z: i32, max_x: i32, max_z: i32) -> bool {
    (min_x >> 4..=max_x >> 4).all(|chunk_x| {
        (min_z >> 4..=max_z >> 4)
            .all(|chunk_z| world.level.is_chunk_loaded(&Vector2::new(chunk_x, chunk_z)))
    })
}

/// Vanilla `Raids.getNearbyRaid(pos, 9216)` over the active raids' centres.
fn nearest_raid(raid_centers: &[(i32, BlockPos)], pos: &BlockPos) -> Option<i32> {
    let mut closest = None;
    let mut closest_dist = Raid::VALID_RAID_RADIUS_SQR;
    for &(id, center) in raid_centers {
        let dist = center.to_f64().squared_distance_to_vec(&pos.to_f64());
        if dist < closest_dist {
            closest = Some(id);
            closest_dist = dist;
        }
    }
    closest
}

/// The raid part of vanilla `Raider.die`, run when a raider dies.
pub fn on_raider_death(
    world: &World,
    raider: &dyn Raider,
    uuid: Uuid,
    killer: Option<&dyn EntityBase>,
) {
    let Some(raid_id) = raider.get_raider_data().raid_id.load() else {
        return;
    };
    let killed_by_player = killer
        .filter(|killer| killer.get_entity().entity_type == &EntityType::PLAYER)
        .map(|killer| killer.get_entity().entity_uuid);
    if let Some(raid) = world
        .raids
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get_mut(raid_id)
    {
        raid.on_raider_death(raider, uuid, killed_by_player);
    }
}

/// Vanilla `Raids`.
pub struct Raids {
    pub raid_map: HashMap<i32, Raid>,
    pub next_id: i32,
    pub tick_counter: i32,
    /// Raiders loaded from disk, joined to their raids at the next raid tick.
    pub pending_members: Vec<PendingRaidMember>,
}

/// A loaded raider that belongs to a raid, vanilla's `Raider.readAdditionalSaveData`.
pub struct PendingRaidMember {
    pub raid_id: i32,
    pub wave: i32,
    pub uuid: Uuid,
    pub leader: bool,
}

impl Default for Raids {
    fn default() -> Self {
        Self {
            raid_map: HashMap::new(),
            next_id: 1,
            tick_counter: 0,
            pending_members: Vec::new(),
        }
    }
}

impl Raids {
    #[must_use]
    pub fn get(&self, raid_id: i32) -> Option<&Raid> {
        self.raid_map.get(&raid_id)
    }

    pub fn get_mut(&mut self, raid_id: i32) -> Option<&mut Raid> {
        self.raid_map.get_mut(&raid_id)
    }

    /// Vanilla `ServerLevel.getRaidAt`.
    #[must_use]
    pub fn get_raid_at(&self, pos: &BlockPos) -> Option<&Raid> {
        self.nearby_raid_id(pos, Raid::VALID_RAID_RADIUS_SQR)
            .and_then(|id| self.raid_map.get(&id))
    }

    pub fn get_raid_at_mut(&mut self, pos: &BlockPos) -> Option<&mut Raid> {
        self.nearby_raid_id(pos, Raid::VALID_RAID_RADIUS_SQR)
            .and_then(|id| self.raid_map.get_mut(&id))
    }

    #[must_use]
    pub fn get_nearby_raid(&self, pos: &BlockPos, max_dist_sqr: f64) -> Option<&Raid> {
        self.nearby_raid_id(pos, max_dist_sqr)
            .and_then(|id| self.raid_map.get(&id))
    }

    /// Vanilla `Raids.getNearbyRaid`: the closest active raid strictly within the distance.
    fn nearby_raid_id(&self, pos: &BlockPos, max_dist_sqr: f64) -> Option<i32> {
        let pos_f64 = pos.to_f64();
        let mut closest = None;
        let mut closest_dist = max_dist_sqr;
        for (&id, raid) in &self.raid_map {
            let dist = raid.center.to_f64().squared_distance_to_vec(&pos_f64);
            if raid.is_active() && dist < closest_dist {
                closest = Some(id);
                closest_dist = dist;
            }
        }
        closest
    }

    /// Vanilla `Raids.createOrExtendRaid`: the raid is centred on the occupied village POIs
    /// around the Raid Omen position and absorbs the player's Raid Omen.
    pub fn create_or_extend_raid(
        &mut self,
        player: &Player,
        raid_position: BlockPos,
        world: &Arc<World>,
    ) -> Option<i32> {
        if player.is_spectator()
            || !world.level_info.load().game_rules.raids
            || !world.dimension.can_start_raid
        {
            return None;
        }
        let pois = world
            .villager_poi
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .village_pois_in_range(raid_position, 64, Occupancy::IsOccupied);
        let raid_center = if pois.is_empty() {
            raid_position
        } else {
            let mut total = Vector3::new(0.0, 0.0, 0.0);
            for pos in &pois {
                total = total.add(&pos.to_f64());
            }
            let scale = 1.0 / pois.len() as f64;
            BlockPos::floored_v(total.multiply(scale, scale, scale))
        };

        let id = if let Some(id) = self.nearby_raid_id(&raid_center, Raid::VALID_RAID_RADIUS_SQR) {
            id
        } else {
            self.next_id += 1;
            let id = self.next_id;
            self.raid_map.insert(
                id,
                Raid::new(id, raid_center, world.level_info.load().difficulty),
            );
            id
        };
        if let Some(raid) = self.raid_map.get_mut(&id)
            && (!raid.is_started() || raid.raid_omen_level < Raid::MAX_RAID_OMEN_LEVEL)
        {
            raid.absorb_raid_omen(player);
        }
        Some(id)
    }

    /// Vanilla `Raid.addWaveMob(..., false)` and `setLeader` for raiders loaded from disk; a
    /// raider whose raid is gone is no longer in a raid.
    fn join_pending_members(&mut self, world: &World) {
        for member in std::mem::take(&mut self.pending_members) {
            if let Some(raid) = self.raid_map.get_mut(&member.raid_id) {
                raid.group_raider_map
                    .entry(member.wave)
                    .or_default()
                    .insert(member.uuid);
                if member.leader {
                    raid.group_to_leader_map.insert(member.wave, member.uuid);
                }
                raid.bossbar_dirty = true;
            } else if let Some(entity) = world.get_entity_by_uuid(member.uuid)
                && let Some(raider) = entity.get_mob().and_then(|mob| mob.as_raider())
            {
                raider.get_raider_data().raid_id.store(None);
            }
        }
    }

    /// Vanilla `Raids.CODEC`.
    #[must_use]
    pub fn to_nbt(&self) -> NbtCompound {
        let mut ids: Vec<&i32> = self.raid_map.keys().collect();
        ids.sort_unstable();
        let raids = ids
            .into_iter()
            .filter_map(|id| self.raid_map.get(id))
            .map(|raid| NbtTag::Compound(raid.to_nbt()))
            .collect::<Vec<_>>();
        let mut nbt = NbtCompound::new();
        // `optionalFieldOf` leaves out the default empty list.
        if !raids.is_empty() {
            nbt.put_list("raids", raids);
        }
        nbt.put_int("next_id", self.next_id);
        nbt.put_int("tick", self.tick_counter);
        nbt
    }

    /// Vanilla `Raids.load`; raids that don't parse are left out.
    #[must_use]
    pub fn from_nbt(nbt: &NbtCompound) -> Self {
        let raid_map = nbt
            .get_list("raids")
            .unwrap_or_default()
            .iter()
            .filter_map(|tag| match tag {
                NbtTag::Compound(raid) => Raid::from_nbt(raid),
                _ => None,
            })
            .map(|raid| (raid.id, raid))
            .collect();
        Self {
            raid_map,
            next_id: nbt.get_int("next_id").unwrap_or(1),
            tick_counter: nbt.get_int("tick").unwrap_or(0),
            pending_members: Vec::new(),
        }
    }

    /// Vanilla `Raids.tick`.
    pub fn tick(&mut self, world: &Arc<World>) {
        self.join_pending_members(world);
        self.tick_counter += 1;
        let raids_enabled = world.level_info.load().game_rules.raids;
        let mut ids: Vec<i32> = self.raid_map.keys().copied().collect();
        ids.sort_unstable();
        for id in ids {
            let raid_centers: Vec<(i32, BlockPos)> = self
                .raid_map
                .iter()
                .filter(|(_, raid)| raid.is_active())
                .map(|(&id, raid)| (id, raid.center))
                .collect();
            let Some(raid) = self.raid_map.get_mut(&id) else {
                continue;
            };
            if !raids_enabled {
                raid.stop(world);
            }
            if raid.is_stopped() {
                self.raid_map.remove(&id);
            } else {
                raid.tick(world, &raid_centers);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use pumpkin_nbt::compound::NbtCompound;
    use pumpkin_nbt::tag::NbtTag;
    use uuid::Uuid;

    use super::{RaidStatus, Raids};

    /// `raids.dat` as vanilla 26.3 saved it during the second wave of a raid.
    fn vanilla_raids() -> NbtCompound {
        let mut raid = NbtCompound::new();
        raid.put_bool("active", false);
        raid.put("center", NbtTag::IntArray(vec![7000, 201, -4]));
        raid.put_int("cooldown_ticks", 0);
        raid.put_int("group_count", 5);
        raid.put_int("groups_spawned", 2);
        raid.put_list(
            "heroes_of_the_village",
            vec![NbtTag::IntArray(vec![
                -1_466_117_591,
                -112_314_537,
                -1_290_531_177,
                1_568_531_417,
            ])],
        );
        raid.put_int("id", 2);
        raid.put_int("post_raid_ticks", 0);
        raid.put_int("raid_omen_level", 1);
        raid.put_bool("started", true);
        raid.put_string("status", "ongoing".to_string());
        raid.put_long("ticks_active", 695);
        raid.put_float("total_health", 144.0);
        let mut data = NbtCompound::new();
        data.put_int("tick", 1498);
        data.put_list("raids", vec![NbtTag::Compound(raid)]);
        data.put_int("next_id", 2);
        data
    }

    #[test]
    fn vanilla_raids_load_and_save_unchanged() {
        let data = vanilla_raids();
        let raids = Raids::from_nbt(&data);
        let raid = raids.get(2).expect("the saved raid loads");
        assert_eq!(raid.status, RaidStatus::Ongoing);
        assert_eq!(raid.groups_spawned, 2);
        assert_eq!(raid.center.0.x, 7000);
        let hero = Uuid::parse_str("a89cd229-f94e-3757-b314-0e975d7de3d9").expect("valid uuid");
        assert!(raid.heroes_of_the_village.contains(&hero));
        assert_eq!(raids.to_nbt(), data);
    }
}
