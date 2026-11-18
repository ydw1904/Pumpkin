use std::sync::{Arc, Mutex, MutexGuard};

use pumpkin_data::attributes::Attributes;
use pumpkin_data::block_properties::{Facing, VaultLikeProperties, VaultState};
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::statistic::StatisticCategory;
use pumpkin_data::world::WorldEvent;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::random::gameplay::game_random;
use pumpkin_world::world::BlockFlags;
use uuid::Uuid;

use super::BlockEntity;
use super::trial_spawner::{EJECT_ACCURACY, EJECT_HEIGHT, PlayerDetector};
use crate::block::blocks::redstone::dispenser::spawn_item;
use crate::entity::EntityBase;
use crate::entity::player::Player;
use crate::world::World;
use crate::world::loot::LootContextParameters;

/// Vanilla `VaultBlockEntity.Server.UNLOCKING_DELAY_TICKS`.
const UNLOCKING_DELAY_TICKS: i64 = 14;
const DISPLAY_CYCLE_TICK_RATE: i64 = 20;
const INSERT_FAIL_SOUND_BUFFER_TICKS: i64 = 15;
/// Vanilla `VaultState`: players are looked for, and items ejected, once a second.
const UPDATE_CONNECTED_PLAYERS_TICK_RATE: i64 = 20;
const DELAY_BETWEEN_EJECTIONS_TICKS: i64 = 20;
const DELAY_BEFORE_FIRST_EJECTION_TICKS: i64 = 20;
/// Vanilla `VaultServerData.MAX_REWARD_PLAYERS`: the oldest rewarded player is forgotten past it.
const MAX_REWARD_PLAYERS: usize = 128;

const DEFAULT_LOOT_TABLE: &str = "minecraft:chests/trial_chambers/reward";
const DEFAULT_ACTIVATION_RANGE: f64 = 4.0;
const DEFAULT_DEACTIVATION_RANGE: f64 = 4.5;

/// Vanilla `VaultConfig`.
#[derive(Clone)]
pub struct VaultConfig {
    pub loot_table: String,
    pub activation_range: f64,
    pub deactivation_range: f64,
    pub key_item: ItemStack,
    pub override_loot_table_to_display: Option<String>,
}

impl Default for VaultConfig {
    fn default() -> Self {
        Self {
            loot_table: DEFAULT_LOOT_TABLE.to_string(),
            activation_range: DEFAULT_ACTIVATION_RANGE,
            deactivation_range: DEFAULT_DEACTIVATION_RANGE,
            key_item: ItemStack::new(1, &Item::TRIAL_KEY),
            override_loot_table_to_display: None,
        }
    }
}

impl VaultConfig {
    /// Vanilla `VaultConfig.CODEC`, whose fields are lenient; a config with the activation
    /// range above the deactivation range is invalid and the default is used.
    fn from_nbt(nbt: &NbtCompound) -> Self {
        let default = Self::default();
        let config = Self {
            loot_table: nbt
                .get_string("loot_table")
                .map_or(default.loot_table, str::to_string),
            activation_range: nbt
                .get_double("activation_range")
                .unwrap_or(default.activation_range),
            deactivation_range: nbt
                .get_double("deactivation_range")
                .unwrap_or(default.deactivation_range),
            key_item: nbt
                .get_compound("key_item")
                .and_then(ItemStack::read_item_stack)
                .unwrap_or(ItemStack::EMPTY.clone()),
            override_loot_table_to_display: nbt
                .get_string("override_loot_table_to_display")
                .map(str::to_string),
        };
        if config.activation_range > config.deactivation_range {
            return Self::default();
        }
        config
    }

    fn write_nbt(&self) -> NbtCompound {
        let mut nbt = NbtCompound::new();
        if self.loot_table != DEFAULT_LOOT_TABLE {
            nbt.put_string("loot_table", self.loot_table.clone());
        }
        if self.activation_range != DEFAULT_ACTIVATION_RANGE {
            nbt.put_double("activation_range", self.activation_range);
        }
        if self.deactivation_range != DEFAULT_DEACTIVATION_RANGE {
            nbt.put_double("deactivation_range", self.deactivation_range);
        }
        if let Some(key) = self.key_item.to_nbt() {
            nbt.put_compound("key_item", key);
        }
        if let Some(table) = &self.override_loot_table_to_display {
            nbt.put_string("override_loot_table_to_display", table.clone());
        }
        nbt
    }
}

fn read_uuids(nbt: &NbtCompound, key: &str) -> Vec<Uuid> {
    let mut uuids: Vec<Uuid> = Vec::new();
    for uuid in nbt
        .get_list(key)
        .into_iter()
        .flatten()
        .filter_map(NbtTag::extract_uuid)
    {
        if !uuids.contains(&uuid) {
            uuids.push(uuid);
        }
    }
    uuids
}

fn put_uuids(nbt: &mut NbtCompound, key: &str, uuids: &[Uuid]) {
    if !uuids.is_empty() {
        nbt.put_list(key, uuids.iter().copied().map(NbtTag::uuid).collect());
    }
}

/// Vanilla `VaultServerData`.
#[derive(Default)]
struct VaultServerData {
    /// In the order the players were rewarded, vanilla's linked set.
    rewarded_players: Vec<Uuid>,
    state_updating_resumes_at: i64,
    /// Ejected from the back.
    items_to_eject: Vec<ItemStack>,
    last_insert_fail_timestamp: i64,
    total_ejections_needed: i32,
    dirty: bool,
}

impl VaultServerData {
    fn from_nbt(nbt: &NbtCompound) -> Self {
        Self {
            rewarded_players: read_uuids(nbt, "rewarded_players"),
            state_updating_resumes_at: nbt.get_long("state_updating_resumes_at").unwrap_or(0),
            items_to_eject: nbt
                .get_list("items_to_eject")
                .into_iter()
                .flatten()
                .filter_map(|tag| match tag {
                    NbtTag::Compound(stack) => ItemStack::read_item_stack(stack),
                    _ => None,
                })
                .collect(),
            last_insert_fail_timestamp: 0,
            total_ejections_needed: nbt.get_int("total_ejections_needed").unwrap_or(0),
            dirty: false,
        }
    }

    fn write_nbt(&self) -> NbtCompound {
        let mut nbt = NbtCompound::new();
        put_uuids(&mut nbt, "rewarded_players", &self.rewarded_players);
        if self.state_updating_resumes_at != 0 {
            nbt.put_long("state_updating_resumes_at", self.state_updating_resumes_at);
        }
        if !self.items_to_eject.is_empty() {
            nbt.put_list(
                "items_to_eject",
                self.items_to_eject
                    .iter()
                    .filter_map(|stack| stack.to_nbt().map(NbtTag::Compound))
                    .collect(),
            );
        }
        if self.total_ejections_needed != 0 {
            nbt.put_int("total_ejections_needed", self.total_ejections_needed);
        }
        nbt
    }

    fn add_to_rewarded_players(&mut self, player: Uuid) {
        if !self.rewarded_players.contains(&player) {
            self.rewarded_players.push(player);
        }
        if self.rewarded_players.len() > MAX_REWARD_PLAYERS {
            self.rewarded_players.remove(0);
        }
        self.dirty = true;
    }

    const fn pause_state_updating_until(&mut self, game_time: i64) {
        self.state_updating_resumes_at = game_time;
        self.dirty = true;
    }

    fn set_items_to_eject(&mut self, items: Vec<ItemStack>) {
        self.total_ejections_needed = items.len() as i32;
        self.items_to_eject = items;
        self.dirty = true;
    }

    fn next_item_to_eject(&self) -> ItemStack {
        self.items_to_eject
            .last()
            .cloned()
            .unwrap_or_else(|| ItemStack::EMPTY.clone())
    }

    fn pop_next_item_to_eject(&mut self) -> ItemStack {
        match self.items_to_eject.pop() {
            Some(stack) => {
                self.dirty = true;
                stack
            }
            None => ItemStack::EMPTY.clone(),
        }
    }

    const fn mark_ejection_finished(&mut self) {
        self.total_ejections_needed = 0;
        self.dirty = true;
    }

    /// Vanilla `ejectionProgress`, which raises the eject sound's pitch item by item.
    fn ejection_progress(&self) -> f32 {
        if self.total_ejections_needed == 1 {
            return 1.0;
        }
        // `1 - Mth.inverseLerp(size, 1, total)`
        let size = self.items_to_eject.len() as f32;
        let total = self.total_ejections_needed as f32;
        1.0 - (size - 1.0) / (total - 1.0)
    }
}

/// Vanilla `VaultSharedData`: what clients are told, for the item shown inside the vault and
/// the particles towards the connected players.
struct VaultSharedData {
    display_item: ItemStack,
    connected_players: Vec<Uuid>,
    connected_particles_range: f64,
    dirty: bool,
}

impl Default for VaultSharedData {
    fn default() -> Self {
        Self {
            display_item: ItemStack::EMPTY.clone(),
            connected_players: Vec::new(),
            connected_particles_range: DEFAULT_DEACTIVATION_RANGE,
            dirty: false,
        }
    }
}

impl VaultSharedData {
    fn from_nbt(nbt: &NbtCompound) -> Self {
        Self {
            display_item: nbt
                .get_compound("display_item")
                .and_then(ItemStack::read_item_stack)
                .unwrap_or(ItemStack::EMPTY.clone()),
            connected_players: read_uuids(nbt, "connected_players"),
            connected_particles_range: nbt
                .get_double("connected_particles_range")
                .unwrap_or(DEFAULT_DEACTIVATION_RANGE),
            dirty: false,
        }
    }

    fn write_nbt(&self) -> NbtCompound {
        let mut nbt = NbtCompound::new();
        if let Some(display_item) = self.display_item.to_nbt() {
            nbt.put_compound("display_item", display_item);
        }
        put_uuids(&mut nbt, "connected_players", &self.connected_players);
        if self.connected_particles_range != DEFAULT_DEACTIVATION_RANGE {
            nbt.put_double("connected_particles_range", self.connected_particles_range);
        }
        nbt
    }

    fn set_display_item(&mut self, stack: ItemStack) {
        if !self.display_item.are_equal(&stack) {
            self.display_item = stack;
            self.dirty = true;
        }
    }

    /// Vanilla `updateConnectedPlayersWithinRange`: players in range who haven't been rewarded.
    fn update_connected_players_within_range(
        &mut self,
        world: &World,
        pos: BlockPos,
        server_data: &VaultServerData,
        limit: f64,
    ) {
        let mut current: Vec<Uuid> = PlayerDetector::IncludingCreativePlayers
            .detect(world, pos, limit, false)
            .into_iter()
            .filter(|uuid| !server_data.rewarded_players.contains(uuid))
            .collect();
        current.sort_unstable();
        let mut connected = self.connected_players.clone();
        connected.sort_unstable();
        if connected != current {
            self.connected_players = current;
            self.dirty = true;
        }
    }
}

#[derive(Default)]
struct VaultData {
    config: VaultConfig,
    server: VaultServerData,
    shared: VaultSharedData,
}

impl VaultData {
    /// Vanilla `canEjectReward`.
    fn can_eject_reward(&self, state: VaultState) -> bool {
        !self.config.key_item.is_empty() && state != VaultState::Inactive
    }

    /// Vanilla `cycleDisplayItemFromLootTable`.
    fn cycle_display_item(&mut self, world: &World, state: VaultState, pos: BlockPos) {
        if !self.can_eject_reward(state) {
            self.shared.set_display_item(ItemStack::EMPTY.clone());
            return;
        }
        let table = self
            .config
            .override_loot_table_to_display
            .as_deref()
            .unwrap_or(&self.config.loot_table);
        let items = world
            .get_loot_table(table)
            .map(|handle| {
                crate::world::loot::generate_loot_from_handle(
                    &handle,
                    game_random().random::<i64>(),
                    &LootContextParameters {
                        world: Some(world),
                        position: Some(pos.to_centered_f64()),
                        ..Default::default()
                    },
                )
            })
            .unwrap_or_default();
        let display = if items.is_empty() {
            ItemStack::EMPTY.clone()
        } else {
            items[game_random().random_range(0..items.len())].clone()
        };
        self.shared.set_display_item(display);
    }

    /// Vanilla `VaultState.updateStateForConnectedPlayers`.
    fn update_state_for_connected_players(
        &mut self,
        world: &World,
        pos: BlockPos,
        game_time: i64,
        range: f64,
    ) -> VaultState {
        self.shared
            .update_connected_players_within_range(world, pos, &self.server, range);
        self.server
            .pause_state_updating_until(game_time + UPDATE_CONNECTED_PLAYERS_TICK_RATE);
        if self.shared.connected_players.is_empty() {
            VaultState::Inactive
        } else {
            VaultState::Active
        }
    }

    /// Vanilla `VaultState.tickAndGetNext`.
    fn tick_and_get_next(
        &mut self,
        world: &Arc<World>,
        pos: BlockPos,
        state: VaultState,
        game_time: i64,
    ) -> VaultState {
        match state {
            VaultState::Inactive => self.update_state_for_connected_players(
                world,
                pos,
                game_time,
                self.config.activation_range,
            ),
            VaultState::Active => self.update_state_for_connected_players(
                world,
                pos,
                game_time,
                self.config.deactivation_range,
            ),
            VaultState::Unlocking => {
                self.server
                    .pause_state_updating_until(game_time + DELAY_BEFORE_FIRST_EJECTION_TICKS);
                VaultState::Ejecting
            }
            VaultState::Ejecting => {
                if self.server.items_to_eject.is_empty() {
                    self.server.mark_ejection_finished();
                    return self.update_state_for_connected_players(
                        world,
                        pos,
                        game_time,
                        self.config.deactivation_range,
                    );
                }
                let progress = self.server.ejection_progress();
                eject_result_item(world, pos, self.server.pop_next_item_to_eject(), progress);
                self.shared
                    .set_display_item(self.server.next_item_to_eject());
                // Vanilla waits as long after the last item as between the others.
                self.server
                    .pause_state_updating_until(game_time + DELAY_BETWEEN_EJECTIONS_TICKS);
                VaultState::Ejecting
            }
        }
    }

    /// Vanilla `VaultState.onTransition`: `from`'s exit, then `to`'s entry.
    fn on_transition(
        &mut self,
        world: &World,
        pos: BlockPos,
        from: VaultState,
        to: VaultState,
        ominous: bool,
    ) {
        if from == VaultState::Ejecting {
            world.play_block_sound(Sound::BlockVaultCloseShutter, SoundCategory::Blocks, pos);
        }
        match to {
            VaultState::Inactive => {
                self.shared.set_display_item(ItemStack::EMPTY.clone());
                world.sync_world_event(
                    WorldEvent::AnimationVaultDeactivate,
                    pos,
                    i32::from(ominous),
                );
            }
            VaultState::Active => {
                if self.shared.display_item.is_empty() {
                    self.cycle_display_item(world, to, pos);
                }
                world.sync_world_event(WorldEvent::AnimationVaultActivate, pos, i32::from(ominous));
            }
            VaultState::Unlocking => {
                world.play_block_sound(Sound::BlockVaultInsertItem, SoundCategory::Blocks, pos);
            }
            VaultState::Ejecting => {
                world.play_block_sound(Sound::BlockVaultOpenShutter, SoundCategory::Blocks, pos);
            }
        }
    }

    /// Vanilla `playInsertFailSound`, at most every 15 ticks.
    fn play_insert_fail_sound(
        &mut self,
        world: &World,
        pos: BlockPos,
        game_time: i64,
        sound: Sound,
    ) {
        if game_time >= self.server.last_insert_fail_timestamp + INSERT_FAIL_SOUND_BUFFER_TICKS {
            world.play_block_sound(sound, SoundCategory::Blocks, pos);
            self.server.last_insert_fail_timestamp = game_time;
        }
    }
}

/// Vanilla `VaultState.ejectResultItem`.
fn eject_result_item(world: &Arc<World>, pos: BlockPos, stack: ItemStack, progress: f32) {
    let origin = Vector3::new(
        f64::from(pos.0.x) + 0.5,
        f64::from(pos.0.y) + EJECT_HEIGHT,
        f64::from(pos.0.z) + 0.5,
    );
    spawn_item(world, stack, EJECT_ACCURACY, Facing::Up, origin);
    world.sync_world_event(WorldEvent::AnimationVaultEjectItem, pos, 0);
    world.play_sound_fine(
        Sound::BlockVaultEjectItem,
        SoundCategory::Blocks,
        &pos.to_centered_f64(),
        1.0,
        0.4f32.mul_add(progress, 0.8),
    );
}

fn game_time(world: &World) -> i64 {
    world
        .level_time
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .query_gametime()
}

/// Vanilla `VaultBlockEntity`.
pub struct VaultBlockEntity {
    pub position: BlockPos,
    data: Mutex<VaultData>,
}

impl VaultBlockEntity {
    pub const ID: &'static str = "minecraft:vault";

    #[must_use]
    pub fn new(position: BlockPos) -> Self {
        Self {
            position,
            data: Mutex::new(VaultData::default()),
        }
    }

    fn data(&self) -> MutexGuard<'_, VaultData> {
        self.data
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Changes the vault's block state, vanilla `setVaultState`.
    fn set_vault_state(
        world: &Arc<World>,
        data: &mut VaultData,
        pos: BlockPos,
        mut props: VaultLikeProperties,
        to: VaultState,
    ) {
        let from = props.vault_state;
        props.vault_state = to;
        world.set_block_state(
            &pos,
            props.to_state_id(&pumpkin_data::Block::VAULT),
            BlockFlags::NOTIFY_ALL,
        );
        data.on_transition(world, pos, from, to, props.ominous);
    }

    /// Vanilla `setChanged` and, when what clients see changed, `sendBlockUpdated`.
    fn flush_changes(&self, world: &World) {
        let (server_dirty, shared_dirty) = {
            let mut data = self.data();
            let dirty = (data.server.dirty, data.shared.dirty);
            data.server.dirty = false;
            data.shared.dirty = false;
            dirty
        };
        if !(server_dirty || shared_dirty) {
            return;
        }
        if let Some(block_entity) = world.get_block_entity(&self.position) {
            if shared_dirty {
                world.update_block_entity(&block_entity);
            } else {
                world.mark_block_entity_changed(&block_entity);
            }
        }
    }

    /// Vanilla `VaultBlockEntity.Server.tryInsertKey`.
    pub fn try_insert_key(&self, world: &Arc<World>, player: &Player, stack: &mut ItemStack) {
        let state_id = world.get_block_state_id(&self.position);
        let props = VaultLikeProperties::from_state_id(state_id);
        let now = game_time(world);
        {
            let mut data = self.data();
            if !data.can_eject_reward(props.vault_state) {
                return;
            }
            let key = &data.config.key_item;
            if !(stack.are_items_and_components_equal(key) && stack.item_count >= key.item_count) {
                data.play_insert_fail_sound(
                    world,
                    self.position,
                    now,
                    Sound::BlockVaultInsertItemFail,
                );
            } else if data
                .server
                .rewarded_players
                .contains(&player.gameprofile.id)
            {
                data.play_insert_fail_sound(
                    world,
                    self.position,
                    now,
                    Sound::BlockVaultRejectRewardedPlayer,
                );
            } else {
                let items =
                    resolve_items_to_eject(world, &data.config, self.position, player, stack);
                if !items.is_empty() {
                    player.increment_stat(StatisticCategory::Used, i32::from(stack.item.id), 1);
                    let key_count = data.config.key_item.item_count;
                    stack.decrement_unless_creative(player.gamemode.load(), key_count);
                    // Vanilla `unlock`.
                    data.server.set_items_to_eject(items);
                    let next = data.server.next_item_to_eject();
                    data.shared.set_display_item(next);
                    data.server
                        .pause_state_updating_until(now + UNLOCKING_DELAY_TICKS);
                    Self::set_vault_state(
                        world,
                        &mut data,
                        self.position,
                        props,
                        VaultState::Unlocking,
                    );
                    data.server.add_to_rewarded_players(player.gameprofile.id);
                    let range = data.config.deactivation_range;
                    let VaultData { server, shared, .. } = &mut *data;
                    shared.update_connected_players_within_range(
                        world,
                        self.position,
                        server,
                        range,
                    );
                }
            }
        }
        self.flush_changes(world);
    }
}

/// Vanilla `resolveItemsToEject`: the reward, rolled with the player's luck and the key.
fn resolve_items_to_eject(
    world: &World,
    config: &VaultConfig,
    pos: BlockPos,
    player: &Player,
    key: &ItemStack,
) -> Vec<ItemStack> {
    world
        .get_loot_table(&config.loot_table)
        .map(|handle| {
            crate::world::loot::generate_loot_from_handle(
                &handle,
                game_random().random::<i64>(),
                &LootContextParameters {
                    world: Some(world),
                    position: Some(pos.to_centered_f64()),
                    luck: player.living_entity.get_attribute_value(&Attributes::LUCK) as f32,
                    this_entity: Some(player as &dyn EntityBase),
                    tool: Some(key.clone()),
                    ..Default::default()
                },
            )
        })
        .unwrap_or_default()
}

impl BlockEntity for VaultBlockEntity {
    fn resource_location(&self) -> &'static str {
        Self::ID
    }

    fn get_position(&self) -> BlockPos {
        self.position
    }

    /// Vanilla `VaultBlockEntity.Server.tick`.
    fn tick(&self, world: &Arc<World>) {
        let state_id = world.get_block_state_id(&self.position);
        if world.get_block(&self.position).id != pumpkin_data::BlockId::VAULT {
            return;
        }
        let props = VaultLikeProperties::from_state_id(state_id);
        let current = props.vault_state;
        let now = game_time(world);
        {
            let mut data = self.data();
            if now % DISPLAY_CYCLE_TICK_RATE == 0 && current == VaultState::Active {
                data.cycle_display_item(world, current, self.position);
            }
            if now >= data.server.state_updating_resumes_at {
                let next = data.tick_and_get_next(world, self.position, current, now);
                if next != current {
                    Self::set_vault_state(world, &mut data, self.position, props, next);
                }
            }
        }
        self.flush_changes(world);
    }

    fn from_nbt(nbt: &NbtCompound, position: BlockPos) -> Self
    where
        Self: Sized,
    {
        Self {
            position,
            data: Mutex::new(VaultData {
                config: nbt
                    .get_compound("config")
                    .map(VaultConfig::from_nbt)
                    .unwrap_or_default(),
                server: nbt
                    .get_compound("server_data")
                    .map(VaultServerData::from_nbt)
                    .unwrap_or_default(),
                shared: nbt
                    .get_compound("shared_data")
                    .map(VaultSharedData::from_nbt)
                    .unwrap_or_default(),
            }),
        }
    }

    fn write_nbt(&self, nbt: &mut NbtCompound) {
        let data = self.data();
        nbt.put_compound("config", data.config.write_nbt());
        nbt.put_compound("shared_data", data.shared.write_nbt());
        nbt.put_compound("server_data", data.server.write_nbt());
    }

    /// Vanilla `getUpdateTag`: clients only get the shared data.
    fn chunk_data_nbt(&self) -> Option<NbtCompound> {
        let mut nbt = NbtCompound::new();
        nbt.put_compound("shared_data", self.data().shared.write_nbt());
        Some(nbt)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
