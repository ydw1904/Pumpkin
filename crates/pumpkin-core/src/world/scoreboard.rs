use rustc_hash::FxHashMap;

use pumpkin_data::scoreboard::ScoreboardDisplaySlot;
use pumpkin_data::translation;
use pumpkin_nbt::{compound::NbtCompound, tag::NbtTag};
use pumpkin_protocol::{
    BClientPacket, ClientPacket, NumberFormat,
    bedrock::client::{
        remove_objective::CRemoveObjective as BRemoveObjective,
        set_display_objective::CSetDisplayObjective as BSetDisplayObjective,
        set_score::{CSetScore as BSetScore, ScoreEntry as BScoreEntry},
    },
    codec::var_int::VarInt,
    java::client::play::{
        CDisplayObjective, CResetScore, CSetPlayerTeam, CUpdateObjectives, CUpdateScore, Mode,
        RenderType, TeamMethod, TeamParameters,
    },
};
use pumpkin_util::text::{TextComponent, color::NamedColor, hover::HoverEvent, style::Style};
use tracing::warn;

use super::World;
use crate::entity::EntityBase;
use crate::entity::player::Player;

pub trait ScoreboardTarget: Send + Sync {
    fn send_editioned<J: ClientPacket + Sync, B: BClientPacket + Sync>(
        &self,
        je_packet: &J,
        be_packet: &B,
    );
    fn send_je<J: ClientPacket + Sync>(&self, je_packet: &J);
}

/// The scoreboard is shared by all dimensions, so like vanilla's `ServerScoreboard`, a world
/// target reaches every player on the server.
impl ScoreboardTarget for World {
    fn send_editioned<J: ClientPacket + Sync, B: BClientPacket + Sync>(
        &self,
        je_packet: &J,
        be_packet: &B,
    ) {
        if let Some(server) = self.server.upgrade() {
            for world in server.worlds.load().iter() {
                world.broadcast_editioned(je_packet, be_packet);
            }
        } else {
            self.broadcast_editioned(je_packet, be_packet);
        }
    }

    fn send_je<J: ClientPacket + Sync>(&self, je_packet: &J) {
        if let Some(server) = self.server.upgrade() {
            server.broadcast_packet_all(je_packet);
        } else {
            self.broadcast_packet_all(je_packet);
        }
    }
}

impl<T: ScoreboardTarget + ?Sized> ScoreboardTarget for &T {
    fn send_editioned<J: ClientPacket + Sync, B: BClientPacket + Sync>(
        &self,
        je_packet: &J,
        be_packet: &B,
    ) {
        (*self).send_editioned(je_packet, be_packet);
    }

    fn send_je<J: ClientPacket + Sync>(&self, je_packet: &J) {
        (*self).send_je(je_packet);
    }
}

impl ScoreboardTarget for std::sync::Arc<World> {
    fn send_editioned<J: ClientPacket + Sync, B: BClientPacket + Sync>(
        &self,
        je_packet: &J,
        be_packet: &B,
    ) {
        (**self).send_editioned(je_packet, be_packet);
    }

    fn send_je<J: ClientPacket + Sync>(&self, je_packet: &J) {
        (**self).send_je(je_packet);
    }
}

impl ScoreboardTarget for Player {
    fn send_editioned<J: ClientPacket + Sync, B: BClientPacket + Sync>(
        &self,
        je_packet: &J,
        be_packet: &B,
    ) {
        self.try_enqueue_packet_editioned(je_packet, be_packet);
    }

    fn send_je<J: ClientPacket + Sync>(&self, je_packet: &J) {
        self.try_send_client_packet(je_packet);
    }
}

impl ScoreboardTarget for std::sync::Arc<Player> {
    fn send_editioned<J: ClientPacket + Sync, B: BClientPacket + Sync>(
        &self,
        je_packet: &J,
        be_packet: &B,
    ) {
        self.try_enqueue_packet_editioned(je_packet, be_packet);
    }

    fn send_je<J: ClientPacket + Sync>(&self, je_packet: &J) {
        self.try_send_client_packet(je_packet);
    }
}

pub struct NoTarget;

impl ScoreboardTarget for NoTarget {
    fn send_editioned<J: ClientPacket + Sync, B: BClientPacket + Sync>(
        &self,
        _je_packet: &J,
        _be_packet: &B,
    ) {
    }

    fn send_je<J: ClientPacket + Sync>(&self, _je_packet: &J) {}
}

#[derive(Clone, Debug, Default)]
pub struct Scoreboard {
    objectives: FxHashMap<String, ScoreboardObjective>,
    display_slots: FxHashMap<ScoreboardDisplaySlot, String>,
    scores: FxHashMap<String, FxHashMap<String, ScoreboardScore>>,
    teams: FxHashMap<String, Team>,
}

impl Scoreboard {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub const fn get_objectives(&self) -> &FxHashMap<String, ScoreboardObjective> {
        &self.objectives
    }

    #[must_use]
    pub fn get_objective(&self, name: &str) -> Option<&ScoreboardObjective> {
        self.objectives.get(name)
    }

    pub fn get_objective_mut(&mut self, name: &str) -> Option<&mut ScoreboardObjective> {
        self.objectives.get_mut(name)
    }

    #[must_use]
    pub const fn get_display_slots(&self) -> &FxHashMap<ScoreboardDisplaySlot, String> {
        &self.display_slots
    }

    #[must_use]
    pub fn get_display_objective(&self, slot: ScoreboardDisplaySlot) -> Option<&str> {
        self.display_slots.get(&slot).map(String::as_str)
    }

    #[must_use]
    pub const fn get_scores(&self) -> &FxHashMap<String, FxHashMap<String, ScoreboardScore>> {
        &self.scores
    }

    #[must_use]
    pub fn get_score(&self, entity_name: &str, objective_name: &str) -> Option<&ScoreboardScore> {
        self.scores.get(objective_name)?.get(entity_name)
    }

    #[must_use]
    pub fn get_score_value(&self, entity_name: &str, objective_name: &str) -> Option<i32> {
        self.get_score(entity_name, objective_name)
            .map(|s| s.value.0)
    }

    #[must_use]
    pub fn get_scores_for_objective(
        &self,
        objective_name: &str,
    ) -> Option<&FxHashMap<String, ScoreboardScore>> {
        self.scores.get(objective_name)
    }

    #[must_use]
    pub fn get_scores_for_entity(&self, entity_name: &str) -> FxHashMap<String, &ScoreboardScore> {
        let mut entity_scores = FxHashMap::default();
        for (obj_name, obj_scores) in &self.scores {
            if let Some(score) = obj_scores.get(entity_name) {
                entity_scores.insert(obj_name.clone(), score);
            }
        }
        entity_scores
    }

    #[must_use]
    pub const fn get_teams(&self) -> &FxHashMap<String, Team> {
        &self.teams
    }

    #[must_use]
    pub fn get_team(&self, name: &str) -> Option<&Team> {
        self.teams.get(name)
    }

    pub fn get_team_mut(&mut self, name: &str) -> Option<&mut Team> {
        self.teams.get_mut(name)
    }

    #[must_use]
    pub fn get_entity_team(&self, entity_name: &str) -> Option<&Team> {
        self.teams
            .values()
            .find(|team| team.players.iter().any(|p| p == entity_name))
    }

    /// Like vanilla's `ServerScoreboard`, clients only learn about objectives shown in a display
    /// slot ("tracked" objectives), so adding one sends nothing.
    pub fn add_objective(
        &mut self,
        _target: &impl ScoreboardTarget,
        objective: ScoreboardObjective,
    ) {
        if self.objectives.contains_key(&objective.name) {
            warn!(
                "Tried to create an objective which already exists: {}",
                &objective.name
            );
            return;
        }
        self.objectives.insert(objective.name.clone(), objective);
    }

    /// Vanilla `ServerScoreboard.onObjectiveChanged`.
    pub fn update_objective(
        &mut self,
        target: &impl ScoreboardTarget,
        objective: ScoreboardObjective,
    ) {
        if !self.objectives.contains_key(&objective.name) {
            warn!(
                "Tried to update an objective which does not exist: {}",
                &objective.name
            );
            return;
        }
        if self.is_tracked(&objective.name) {
            target.send_je(&objective.packet(Mode::Update));
        }
        self.objectives.insert(objective.name.clone(), objective);
    }

    /// Whether the objective is shown in a display slot, which is when vanilla's
    /// `ServerScoreboard` tracks it for clients.
    #[must_use]
    pub fn is_tracked(&self, objective_name: &str) -> bool {
        self.display_slots
            .values()
            .any(|name| name == objective_name)
    }

    fn display_packets(
        &self,
        slot: ScoreboardDisplaySlot,
        objective_name: Option<&str>,
    ) -> (CDisplayObjective, BSetDisplayObjective) {
        let name = objective_name.unwrap_or_default();
        let display_name = objective_name
            .and_then(|name| self.objectives.get(name))
            .map_or_else(|| name.to_string(), |o| o.display_name.clone().get_text());
        let slot_str = match slot {
            ScoreboardDisplaySlot::List => "list",
            ScoreboardDisplaySlot::BelowName => "belowname",
            _ => "sidebar",
        };
        (
            CDisplayObjective::new(slot, name.to_string()),
            BSetDisplayObjective {
                display_slot_name: slot_str.to_string(),
                objective_name: name.to_string(),
                objective_display_name: display_name,
                criteria_name: "dummy".to_string(),
                sort_order: VarInt(0),
            },
        )
    }

    fn send_display(
        &self,
        target: &impl ScoreboardTarget,
        slot: ScoreboardDisplaySlot,
        objective_name: Option<&str>,
    ) {
        let (je, be) = self.display_packets(slot, objective_name);
        target.send_editioned(&je, &be);
    }

    /// Vanilla `ServerScoreboard.getStartTrackingPackets`.
    fn send_start_tracking(&self, target: &impl ScoreboardTarget, objective_name: &str) {
        let Some(objective) = self.objectives.get(objective_name) else {
            return;
        };
        target.send_je(&objective.packet(Mode::Add));
        for slot in ALL_DISPLAY_SLOTS {
            if self.get_display_objective(slot) == Some(objective_name) {
                self.send_display(target, slot, Some(objective_name));
            }
        }
        if let Some(scores) = self.scores.get(objective_name) {
            for score in scores.values() {
                let (je, be) = score.packets();
                target.send_editioned(&je, &be);
            }
        }
    }

    /// Vanilla `ServerScoreboard.getStopTrackingPackets`.
    fn send_stop_tracking(&self, target: &impl ScoreboardTarget, objective_name: &str) {
        target.send_editioned(
            &CUpdateObjectives::new(
                objective_name.to_string(),
                Mode::Remove,
                TextComponent::empty(),
                RenderType::Integer,
                None,
            ),
            &BRemoveObjective {
                objective_name: objective_name.to_string(),
            },
        );
        for slot in ALL_DISPLAY_SLOTS {
            if self.get_display_objective(slot) == Some(objective_name) {
                self.send_display(target, slot, Some(objective_name));
            }
        }
    }

    /// Vanilla `ServerScoreboard.setDisplayObjective`.
    pub fn set_display_objective(
        &mut self,
        target: &impl ScoreboardTarget,
        slot: ScoreboardDisplaySlot,
        objective_name: Option<&str>,
    ) {
        if let Some(name) = objective_name
            && !self.objectives.contains_key(name)
        {
            warn!("Tried to display an objective which does not exist: {name}");
            return;
        }
        let old = self.display_slots.get(&slot).cloned();
        let was_tracked = objective_name.is_some_and(|name| self.is_tracked(name));
        if let Some(name) = objective_name {
            self.display_slots.insert(slot, name.to_string());
        } else {
            self.display_slots.remove(&slot);
        }

        if let Some(old) = old.as_deref()
            && Some(old) != objective_name
        {
            if self.is_tracked(old) {
                self.send_display(target, slot, objective_name);
            } else {
                self.send_stop_tracking(target, old);
            }
        }
        if let Some(name) = objective_name {
            if was_tracked {
                self.send_display(target, slot, Some(name));
            } else {
                self.send_start_tracking(target, name);
            }
        }
    }

    pub fn clear_display_objective(
        &mut self,
        target: &impl ScoreboardTarget,
        slot: ScoreboardDisplaySlot,
    ) {
        self.set_display_objective(target, slot, None);
    }

    /// Vanilla `Scoreboard.removeObjective`: the objective leaves its display slots first,
    /// which stops tracking it.
    pub fn remove_objective(&mut self, target: &impl ScoreboardTarget, name: &str) {
        if !self.objectives.contains_key(name) {
            warn!(
                "Tried to remove an objective which does not exist: {}",
                name
            );
            return;
        }
        for slot in ALL_DISPLAY_SLOTS {
            if self.get_display_objective(slot) == Some(name) {
                self.set_display_objective(target, slot, None);
            }
        }
        self.objectives.remove(name);
        self.scores.remove(name);
    }

    /// Stores the score and, when its objective is tracked, sends it like vanilla's
    /// `ServerScoreboard.onScoreChanged`.
    pub fn update_score(&mut self, target: &impl ScoreboardTarget, score: ScoreboardScore) {
        if !self.objectives.contains_key(&score.objective_name) {
            warn!(
                "Tried to place a score into an objective which does not exist: {}",
                &score.objective_name
            );
            return;
        }

        if self.is_tracked(&score.objective_name) {
            let (je, be) = score.packets();
            target.send_editioned(&je, &be);
        }

        self.scores
            .entry(score.objective_name.clone())
            .or_default()
            .insert(score.entity_name.clone(), score);
    }

    pub fn set_score_value(
        &mut self,
        target: &impl ScoreboardTarget,
        entity_name: impl Into<String>,
        objective_name: impl Into<String>,
        value: i32,
    ) {
        self.set_holder_score(
            target,
            &entity_name.into(),
            None,
            &objective_name.into(),
            value,
        );
    }

    /// Vanilla `ScoreAccess.set`: `holder_display` is the holder entity's display name, which
    /// `displayAutoUpdate` objectives copy onto the score. Nothing is sent when nothing changed.
    pub fn set_holder_score(
        &mut self,
        target: &impl ScoreboardTarget,
        holder: &str,
        holder_display: Option<&TextComponent>,
        objective_name: &str,
        value: i32,
    ) {
        let auto_update = self
            .objectives
            .get(objective_name)
            .is_some_and(|o| o.display_auto_update);
        let existing = self.get_score(holder, objective_name);
        let mut changed = existing.is_none();
        let mut score = existing
            .cloned()
            .unwrap_or_else(|| ScoreboardScore::new(holder, objective_name, VarInt(0), None, None));
        if auto_update
            && let Some(display) = holder_display
            && score.display_name.as_ref() != Some(display)
        {
            score.display_name = Some(display.clone());
            changed = true;
        }
        if score.value.0 != value {
            score.value = VarInt(value);
            changed = true;
        }
        if changed {
            self.update_score(target, score);
        }
    }

    /// Vanilla `ScoreAccess.lock` / `unlock`, which change the score without syncing it.
    pub fn set_score_locked(&mut self, holder: &str, objective_name: &str, locked: bool) {
        if let Some(score) = self
            .scores
            .get_mut(objective_name)
            .and_then(|scores| scores.get_mut(holder))
        {
            score.locked = locked;
        }
    }

    pub fn add_score(
        &mut self,
        target: &impl ScoreboardTarget,
        entity_name: impl Into<String>,
        objective_name: impl Into<String>,
        delta: i32,
    ) -> i32 {
        let entity_s = entity_name.into();
        let obj_s = objective_name.into();
        let current_val = self.get_score_value(&entity_s, &obj_s).unwrap_or(0);
        let new_val = current_val.wrapping_add(delta);
        self.set_score_value(target, entity_s, obj_s, new_val);
        new_val
    }

    fn holder_has_scores(&self, entity_name: &str) -> bool {
        self.scores
            .values()
            .any(|scores| scores.contains_key(entity_name))
    }

    fn bedrock_reset_packet(entity_name: &str, objective_names: &[&str]) -> BSetScore {
        BSetScore {
            action: VarInt(1), // Remove
            entries: objective_names
                .iter()
                .map(|objective_name| BScoreEntry {
                    scoreboard_id: entity_name.as_ptr() as i64,
                    objective_name: (*objective_name).to_string(),
                    score: VarInt(0),
                    entry_type: VarInt(3),
                    entity_unique_id: 0,
                    custom_name: entity_name.to_string(),
                })
                .collect(),
        }
    }

    /// Vanilla `Scoreboard.resetSinglePlayerScore`: a holder losing its last score is reset
    /// as a whole.
    pub fn remove_score(
        &mut self,
        target: &impl ScoreboardTarget,
        entity_name: &str,
        objective_name: &str,
    ) {
        let removed = self
            .scores
            .get_mut(objective_name)
            .and_then(|scores| scores.remove(entity_name))
            .is_some();
        if !removed {
            return;
        }
        let be_packet = Self::bedrock_reset_packet(entity_name, &[objective_name]);
        if !self.holder_has_scores(entity_name) {
            if !self.display_slots.is_empty() {
                target.send_editioned(&CResetScore::new(entity_name.to_string(), None), &be_packet);
            }
        } else if self.is_tracked(objective_name) {
            target.send_editioned(
                &CResetScore::new(entity_name.to_string(), Some(objective_name.to_string())),
                &be_packet,
            );
        }
    }

    /// Vanilla `Scoreboard.resetAllPlayerScores`.
    pub fn reset_scores_for_entity(&mut self, target: &impl ScoreboardTarget, entity_name: &str) {
        let mut removed = Vec::new();
        for (objective_name, scores) in &mut self.scores {
            if scores.remove(entity_name).is_some() {
                removed.push(objective_name.as_str());
            }
        }
        if !removed.is_empty() && !self.display_slots.is_empty() {
            target.send_editioned(
                &CResetScore::new(entity_name.to_string(), None),
                &Self::bedrock_reset_packet(entity_name, &removed),
            );
        }
    }

    /// Vanilla `Scoreboard.forAllObjectives`: applies `op` to the holder's score in every
    /// objective with the criterion, creating missing scores.
    pub fn for_all_objectives(
        &mut self,
        target: &impl ScoreboardTarget,
        criterion: &str,
        holder: &str,
        holder_display: Option<&TextComponent>,
        op: impl Fn(i32) -> i32,
    ) {
        let objectives: Vec<String> = self
            .objectives
            .values()
            .filter(|objective| objective.criterion == criterion)
            .map(|objective| objective.name.clone())
            .collect();
        for objective in objectives {
            let value = op(self.get_score_value(holder, &objective).unwrap_or(0));
            self.set_holder_score(target, holder, holder_display, &objective, value);
        }
    }

    /// Whether some objective uses a stat criterion like `minecraft.mined:minecraft.stone`,
    /// so stat increments can skip building criterion names when none does.
    #[must_use]
    pub fn has_stat_objectives(&self) -> bool {
        self.objectives
            .values()
            .any(|objective| objective.criterion.contains(':'))
    }

    /// Vanilla `Scoreboard.getTrackedPlayers`: every holder with a score.
    #[must_use]
    pub fn tracked_holders(&self) -> Vec<&str> {
        let mut holders: Vec<&str> = self
            .scores
            .values()
            .flat_map(|scores| scores.keys().map(String::as_str))
            .collect();
        holders.sort_unstable();
        holders.dedup();
        holders
    }

    pub fn add_team(&mut self, target: &impl ScoreboardTarget, team: Team) {
        if self.teams.contains_key(&team.name) {
            warn!("Tried to create Team which already exists, {}", team.name);
            return;
        }

        target.send_je(&CSetPlayerTeam {
            team_name: team.name.clone(),
            method: TeamMethod::Create,
            parameters: Some(team.parameters()),
            players: team.players.clone().into(),
        });

        self.teams.insert(team.name.clone(), team);
    }

    pub fn create_team(&mut self, target: &impl ScoreboardTarget, team: Team) {
        self.add_team(target, team);
    }

    pub fn update_team(&mut self, target: &impl ScoreboardTarget, team: Team) {
        if !self.teams.contains_key(&team.name) {
            warn!("Tried to update Team which does not exist, {}", team.name);
            return;
        }

        target.send_je(&CSetPlayerTeam {
            team_name: team.name.clone(),
            method: TeamMethod::Update,
            parameters: Some(team.parameters()),
            players: Box::new([]),
        });

        self.teams.insert(team.name.clone(), team);
    }

    pub fn remove_team(&mut self, target: &impl ScoreboardTarget, name: &str) {
        if !self.teams.contains_key(name) {
            warn!("Tried to remove Team which does not exist, {}", name);
            return;
        }

        target.send_je(&CSetPlayerTeam {
            team_name: name.to_string(),
            method: TeamMethod::Remove,
            parameters: None,
            players: Box::new([]),
        });

        self.teams.remove(name);
    }

    /// Vanilla `ServerScoreboard.addPlayerToTeam`: the player leaves its old team first.
    /// Returns whether the player joined, which is false when it already was a member.
    pub fn add_player_to_team(
        &mut self,
        target: &impl ScoreboardTarget,
        team_name: &str,
        player: String,
    ) -> bool {
        if !self.teams.contains_key(team_name) {
            warn!(
                "Tried to add player to Team which does not exist, {}",
                team_name
            );
            return false;
        }
        if let Some(old_team) = self
            .get_entity_team(&player)
            .map(|team| team.name.clone())
            .filter(|old_team| old_team != team_name)
        {
            self.remove_player_from_team(target, &old_team, &player);
        }
        let Some(team) = self.teams.get_mut(team_name) else {
            return false;
        };

        if team.players.contains(&player) {
            return false;
        }

        target.send_je(&CSetPlayerTeam {
            team_name: team_name.to_string(),
            method: TeamMethod::AddPlayers,
            parameters: None,
            players: vec![player.clone()].into(),
        });

        team.players.push(player);
        true
    }

    /// Returns whether the player was a member.
    pub fn remove_player_from_team(
        &mut self,
        target: &impl ScoreboardTarget,
        team_name: &str,
        player: &str,
    ) -> bool {
        let Some(team) = self.teams.get_mut(team_name) else {
            warn!(
                "Tried to remove player from Team which does not exist, {}",
                team_name
            );
            return false;
        };

        if !team.players.iter().any(|p| p == player) {
            return false;
        }

        target.send_je(&CSetPlayerTeam {
            team_name: team_name.to_string(),
            method: TeamMethod::RemovePlayers,
            parameters: None,
            players: vec![player.to_string()].into(),
        });

        team.players.retain(|p| p != player);
        true
    }

    pub fn clear_team_players(&mut self, target: &impl ScoreboardTarget, team_name: &str) {
        let Some(team) = self.teams.get_mut(team_name) else {
            warn!(
                "Tried to clear players from Team which does not exist, {}",
                team_name
            );
            return;
        };

        if team.players.is_empty() {
            return;
        }

        let players_to_remove = team.players.clone();
        target.send_je(&CSetPlayerTeam {
            team_name: team_name.to_string(),
            method: TeamMethod::RemovePlayers,
            parameters: None,
            players: players_to_remove.into(),
        });

        team.players.clear();
    }

    /// Vanilla `PlayerList.updateEntireScoreboard`: teams, then each displayed objective once.
    pub fn send_to_player(&self, player: &Player) {
        for team in self.teams.values() {
            player.try_send_client_packet(&CSetPlayerTeam {
                team_name: team.name.clone(),
                method: TeamMethod::Create,
                parameters: Some(team.parameters()),
                players: team.players.clone().into(),
            });
        }
        let mut sent: Vec<&str> = Vec::new();
        for slot in ALL_DISPLAY_SLOTS {
            if let Some(name) = self.get_display_objective(slot)
                && !sent.contains(&name)
            {
                self.send_start_tracking(player, name);
                sent.push(name);
            }
        }
    }
}

/// Names of vanilla's built-in `ObjectiveCriteria` that the server updates itself.
pub mod criteria {
    pub const TRIGGER: &str = "trigger";
    pub const DEATH_COUNT: &str = "deathCount";
    pub const KILL_COUNT_PLAYERS: &str = "playerKillCount";
    pub const KILL_COUNT_ALL: &str = "totalKillCount";
    pub const HEALTH: &str = "health";
    pub const FOOD: &str = "food";
    pub const AIR: &str = "air";
    pub const ARMOR: &str = "armor";
    pub const EXPERIENCE: &str = "xp";
    pub const LEVEL: &str = "level";
    pub const TEAM_KILL_PREFIX: &str = "teamkill.";
    pub const KILLED_BY_TEAM_PREFIX: &str = "killedByTeam.";
}

/// Vanilla `Scoreboard.forAllObjectives` with an entity as the score holder.
pub fn for_all_entity_objectives(
    holder: &dyn EntityBase,
    criterion: &str,
    op: impl Fn(i32) -> i32,
) {
    let world = holder.get_entity().world.load_full();
    let needs_display = {
        let scoreboard = world
            .scoreboard
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut matching = scoreboard
            .objectives
            .values()
            .filter(|objective| objective.criterion == criterion)
            .peekable();
        if matching.peek().is_none() {
            return;
        }
        matching.any(|objective| objective.display_auto_update)
    };
    // The display name reads the holder's team, so it is built without the scoreboard locked.
    let display = needs_display.then(|| holder.get_display_name());
    world
        .scoreboard
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .for_all_objectives(
            &*world,
            criterion,
            &holder.get_scoreboard_name(),
            display.as_ref(),
            op,
        );
}

/// Vanilla `ServerPlayer.handleTeamKill`: `source` scores in the `<prefix><color>` objectives
/// of `target`'s team color.
pub fn handle_team_kill(source: &dyn EntityBase, target: &dyn EntityBase, prefix: &str) {
    if let Some(color) = target.get_team().and_then(|team| team.color) {
        for_all_entity_objectives(source, &format!("{prefix}{}", color.name()), |v| {
            v.wrapping_add(1)
        });
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScoreboardObjective {
    pub name: String,
    pub display_name: TextComponent,
    pub render_type: RenderType,
    pub number_format: Option<NumberFormat>,
    pub criterion: String,
    pub display_auto_update: bool,
}

impl ScoreboardObjective {
    /// Vanilla `Objective.getFormattedDisplayName`.
    #[must_use]
    pub fn formatted_display_name(&self) -> TextComponent {
        self.display_name
            .clone()
            .hover_event(HoverEvent::show_text(TextComponent::text(
                self.name.clone(),
            )))
            .wrap_in_square_brackets()
    }

    fn packet(&self, mode: Mode) -> CUpdateObjectives {
        CUpdateObjectives::new(
            self.name.clone(),
            mode,
            self.display_name.clone(),
            self.render_type,
            self.number_format.clone(),
        )
    }

    #[must_use]
    pub fn new(
        name: impl Into<String>,
        display_name: TextComponent,
        render_type: RenderType,
        number_format: Option<NumberFormat>,
        criterion: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            display_name,
            render_type,
            number_format,
            criterion: criterion.into(),
            display_auto_update: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScoreboardScore {
    pub entity_name: String,
    pub objective_name: String,
    pub value: VarInt,
    pub display_name: Option<TextComponent>,
    pub number_format: Option<NumberFormat>,
    pub locked: bool,
}

impl ScoreboardScore {
    fn packets(&self) -> (CUpdateScore, BSetScore) {
        (
            CUpdateScore::new(
                self.entity_name.clone(),
                self.objective_name.clone(),
                self.value,
                self.display_name.clone(),
                self.number_format.clone(),
            ),
            BSetScore {
                action: VarInt(0), // Change
                entries: vec![BScoreEntry {
                    scoreboard_id: self.entity_name.as_ptr() as i64,
                    objective_name: self.objective_name.clone(),
                    score: self.value,
                    entry_type: VarInt(3), // Fake player / Literal
                    entity_unique_id: 0,
                    custom_name: self.entity_name.clone(),
                }],
            },
        )
    }

    #[must_use]
    pub fn new(
        entity_name: impl Into<String>,
        objective_name: impl Into<String>,
        value: VarInt,
        display_name: Option<TextComponent>,
        number_format: Option<NumberFormat>,
    ) -> Self {
        Self {
            entity_name: entity_name.into(),
            objective_name: objective_name.into(),
            value,
            display_name,
            number_format,
            locked: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameTagVisibility {
    Always,
    Never,
    HideForOtherTeams,
    HideForOwnTeam,
}

impl NameTagVisibility {
    #[must_use]
    pub const fn to_str(&self) -> &'static str {
        match self {
            Self::Always => "always",
            Self::Never => "never",
            Self::HideForOtherTeams => "hideForOtherTeams",
            Self::HideForOwnTeam => "hideForOwnTeam",
        }
    }

    /// Vanilla `Team.Visibility.getDisplayName`.
    #[must_use]
    pub fn display_name(&self) -> TextComponent {
        TextComponent::translate(
            match self {
                Self::Always => translation::java::TEAM_VISIBILITY_ALWAYS,
                Self::Never => translation::java::TEAM_VISIBILITY_NEVER,
                Self::HideForOtherTeams => translation::java::TEAM_VISIBILITY_HIDEFOROTHERTEAMS,
                Self::HideForOwnTeam => translation::java::TEAM_VISIBILITY_HIDEFOROWNTEAM,
            },
            [],
        )
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        [
            Self::Always,
            Self::Never,
            Self::HideForOtherTeams,
            Self::HideForOwnTeam,
        ]
        .into_iter()
        .find(|v| v.to_str() == name)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CollisionRule {
    Always,
    Never,
    PushOtherTeams,
    PushOwnTeam,
}

impl CollisionRule {
    #[must_use]
    pub const fn to_str(&self) -> &'static str {
        match self {
            Self::Always => "always",
            Self::Never => "never",
            Self::PushOtherTeams => "pushOtherTeams",
            Self::PushOwnTeam => "pushOwnTeam",
        }
    }

    /// Vanilla `Team.CollisionRule.getDisplayName`.
    #[must_use]
    pub fn display_name(&self) -> TextComponent {
        TextComponent::translate(
            match self {
                Self::Always => translation::java::TEAM_COLLISION_ALWAYS,
                Self::Never => translation::java::TEAM_COLLISION_NEVER,
                Self::PushOtherTeams => translation::java::TEAM_COLLISION_PUSHOTHERTEAMS,
                Self::PushOwnTeam => translation::java::TEAM_COLLISION_PUSHOWNTEAM,
            },
            [],
        )
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        [
            Self::Always,
            Self::Never,
            Self::PushOtherTeams,
            Self::PushOwnTeam,
        ]
        .into_iter()
        .find(|v| v.to_str() == name)
    }
}

#[derive(Clone, Debug)]
pub struct Team {
    pub name: String,
    pub display_name: TextComponent,
    pub options: i8,
    pub nametag_visibility: NameTagVisibility,
    pub death_message_visibility: NameTagVisibility,
    pub collision_rule: CollisionRule,
    /// `None` is a team without a color, like vanilla's empty `Optional<TeamColor>`.
    pub color: Option<NamedColor>,
    pub player_prefix: TextComponent,
    pub player_suffix: TextComponent,
    pub players: Vec<String>,
}

impl Team {
    pub(crate) const ALLOW_FRIENDLY_FIRE: i8 = 0x01;
    pub(crate) const SEE_FRIENDLY_INVISIBLES: i8 = 0x02;

    #[must_use]
    pub const fn is_allow_friendly_fire(&self) -> bool {
        self.options & Self::ALLOW_FRIENDLY_FIRE != 0
    }

    /// Vanilla `PlayerTeam.getFormattedDisplayName`.
    #[must_use]
    pub fn formatted_display_name(&self) -> TextComponent {
        let name = self
            .display_name
            .clone()
            .insertion(self.name.clone())
            .hover_event(HoverEvent::show_text(TextComponent::text(
                self.name.clone(),
            )))
            .wrap_in_square_brackets();
        match self.color {
            Some(color) => name.color_named(color),
            None => name,
        }
    }

    /// Vanilla `PlayerTeam.getFormattedName`.
    #[must_use]
    pub fn formatted_name(&self, member_name: TextComponent) -> TextComponent {
        let name = TextComponent::empty()
            .add_child(self.player_prefix.clone())
            .add_child(member_name)
            .add_child(self.player_suffix.clone());
        match self.color {
            Some(color) => name.color_named(color),
            None => name,
        }
    }

    /// Vanilla `PlayerTeam.formatNameForTeam`.
    #[must_use]
    pub fn format_name_for_team(team: Option<&Self>, name: TextComponent) -> TextComponent {
        match team {
            Some(team) => team.formatted_name(name),
            None => name,
        }
    }

    const fn parameters(&self) -> TeamParameters<'_> {
        TeamParameters {
            display_name: &self.display_name,
            options: self.options,
            nametag_visibility: self.nametag_visibility.to_str(),
            collision_rule: self.collision_rule.to_str(),
            color: self.color,
            player_prefix: &self.player_prefix,
            player_suffix: &self.player_suffix,
        }
    }

    /// Vanilla `PlayerTeam.Packed`.
    fn pack(&self) -> NbtCompound {
        let mut nbt = NbtCompound::new();
        nbt.put_string("Name", self.name.clone());
        nbt.put("DisplayName", self.display_name.0.to_nbt_tag());
        if let Some(color) = self.color {
            nbt.put_string("TeamColor", color.name().to_string());
        }
        // Like vanilla's `optionalFieldOf` codecs, fields at their default are left out.
        if !self.is_allow_friendly_fire() {
            nbt.put_bool("AllowFriendlyFire", false);
        }
        if self.options & Self::SEE_FRIENDLY_INVISIBLES == 0 {
            nbt.put_bool("SeeFriendlyInvisibles", false);
        }
        for (key, text) in [
            ("MemberNamePrefix", &self.player_prefix),
            ("MemberNameSuffix", &self.player_suffix),
        ] {
            if *text != TextComponent::empty() {
                nbt.put(key, text.0.to_nbt_tag());
            }
        }
        for (key, visibility) in [
            ("NameTagVisibility", self.nametag_visibility),
            ("DeathMessageVisibility", self.death_message_visibility),
        ] {
            if visibility != NameTagVisibility::Always {
                nbt.put_string(key, visibility.to_str().to_string());
            }
        }
        if self.collision_rule != CollisionRule::Always {
            nbt.put_string("CollisionRule", self.collision_rule.to_str().to_string());
        }
        if !self.players.is_empty() {
            nbt.put_list(
                "Players",
                self.players
                    .iter()
                    .map(|p| NbtTag::String(p.as_str().into()))
                    .collect(),
            );
        }
        nbt
    }

    fn load(nbt: &NbtCompound) -> Option<Self> {
        let name = nbt.get_string("Name")?.to_string();
        let text = |key: &str| nbt.get(key).map(TextComponent::from_nbt);
        let mut options = 0;
        if nbt.get_bool("AllowFriendlyFire").unwrap_or(true) {
            options |= Self::ALLOW_FRIENDLY_FIRE;
        }
        if nbt.get_bool("SeeFriendlyInvisibles").unwrap_or(true) {
            options |= Self::SEE_FRIENDLY_INVISIBLES;
        }
        let visibility = |key: &str| {
            nbt.get_string(key)
                .and_then(NameTagVisibility::from_name)
                .unwrap_or(NameTagVisibility::Always)
        };
        Some(Self {
            display_name: text("DisplayName").unwrap_or_else(|| TextComponent::text(name.clone())),
            options,
            nametag_visibility: visibility("NameTagVisibility"),
            death_message_visibility: visibility("DeathMessageVisibility"),
            collision_rule: nbt
                .get_string("CollisionRule")
                .and_then(CollisionRule::from_name)
                .unwrap_or(CollisionRule::Always),
            color: nbt
                .get_string("TeamColor")
                .and_then(|c| NamedColor::try_from(c).ok()),
            player_prefix: text("MemberNamePrefix").unwrap_or_else(TextComponent::empty),
            player_suffix: text("MemberNameSuffix").unwrap_or_else(TextComponent::empty),
            players: nbt
                .get_list("Players")
                .unwrap_or_default()
                .iter()
                .filter_map(|p| p.extract_string().map(str::to_string))
                .collect(),
            name,
        })
    }
}

/// Vanilla `DisplaySlot` serialized names.
pub(crate) const fn display_slot_name(slot: ScoreboardDisplaySlot) -> &'static str {
    match slot {
        ScoreboardDisplaySlot::List => "list",
        ScoreboardDisplaySlot::Sidebar => "sidebar",
        ScoreboardDisplaySlot::BelowName => "below_name",
        ScoreboardDisplaySlot::TeamBlack => "sidebar.team.black",
        ScoreboardDisplaySlot::TeamDarkBlue => "sidebar.team.dark_blue",
        ScoreboardDisplaySlot::TeamDarkGreen => "sidebar.team.dark_green",
        ScoreboardDisplaySlot::TeamDarkAqua => "sidebar.team.dark_aqua",
        ScoreboardDisplaySlot::TeamDarkRed => "sidebar.team.dark_red",
        ScoreboardDisplaySlot::TeamDarkPurple => "sidebar.team.dark_purple",
        ScoreboardDisplaySlot::TeamGold => "sidebar.team.gold",
        ScoreboardDisplaySlot::TeamGray => "sidebar.team.gray",
        ScoreboardDisplaySlot::TeamDarkGray => "sidebar.team.dark_gray",
        ScoreboardDisplaySlot::TeamBlue => "sidebar.team.blue",
        ScoreboardDisplaySlot::TeamGreen => "sidebar.team.green",
        ScoreboardDisplaySlot::TeamAqua => "sidebar.team.aqua",
        ScoreboardDisplaySlot::TeamRed => "sidebar.team.red",
        ScoreboardDisplaySlot::TeamLightPurple => "sidebar.team.light_purple",
        ScoreboardDisplaySlot::TeamYellow => "sidebar.team.yellow",
        ScoreboardDisplaySlot::TeamWhite => "sidebar.team.white",
    }
}

pub(crate) const ALL_DISPLAY_SLOTS: [ScoreboardDisplaySlot; 19] = [
    ScoreboardDisplaySlot::List,
    ScoreboardDisplaySlot::Sidebar,
    ScoreboardDisplaySlot::BelowName,
    ScoreboardDisplaySlot::TeamBlack,
    ScoreboardDisplaySlot::TeamDarkBlue,
    ScoreboardDisplaySlot::TeamDarkGreen,
    ScoreboardDisplaySlot::TeamDarkAqua,
    ScoreboardDisplaySlot::TeamDarkRed,
    ScoreboardDisplaySlot::TeamDarkPurple,
    ScoreboardDisplaySlot::TeamGold,
    ScoreboardDisplaySlot::TeamGray,
    ScoreboardDisplaySlot::TeamDarkGray,
    ScoreboardDisplaySlot::TeamBlue,
    ScoreboardDisplaySlot::TeamGreen,
    ScoreboardDisplaySlot::TeamAqua,
    ScoreboardDisplaySlot::TeamRed,
    ScoreboardDisplaySlot::TeamLightPurple,
    ScoreboardDisplaySlot::TeamYellow,
    ScoreboardDisplaySlot::TeamWhite,
];

/// The style fields of a component, which is how vanilla's `Style.Serializer.MAP_CODEC` saves a style.
fn style_to_nbt(style: &Style) -> NbtCompound {
    let mut component = TextComponent::text("");
    *component.0.style = style.clone();
    let mut nbt = component.0.to_nbt_compound();
    nbt.child_tags.remove("text");
    nbt
}

pub(crate) fn style_from_nbt(nbt: &NbtCompound) -> Style {
    let mut nbt = nbt.clone();
    nbt.put_string("text", String::new());
    *TextComponent::from_nbt(&NbtTag::Compound(nbt)).0.style
}

/// Vanilla `NumberFormatTypes.CODEC`.
fn number_format_to_nbt(format: &NumberFormat) -> NbtCompound {
    match format {
        NumberFormat::Blank => {
            let mut nbt = NbtCompound::new();
            nbt.put_string("type", "minecraft:blank".to_string());
            nbt
        }
        NumberFormat::Styled(style) => {
            let mut nbt = style_to_nbt(style);
            nbt.put_string("type", "minecraft:styled".to_string());
            nbt
        }
        NumberFormat::Fixed(value) => {
            let mut nbt = NbtCompound::new();
            nbt.put_string("type", "minecraft:fixed".to_string());
            nbt.put("value", value.0.to_nbt_tag());
            nbt
        }
    }
}

fn number_format_from_nbt(nbt: &NbtCompound) -> Option<NumberFormat> {
    match nbt
        .get_string("type")?
        .strip_prefix("minecraft:")
        .unwrap_or_default()
    {
        "blank" => Some(NumberFormat::Blank),
        "styled" => Some(NumberFormat::Styled(style_from_nbt(nbt))),
        "fixed" => Some(NumberFormat::Fixed(TextComponent::from_nbt(
            nbt.get("value")?,
        ))),
        _ => None,
    }
}

impl Scoreboard {
    /// Loads vanilla's `ScoreboardSaveData.Packed`, in `ServerScoreboard.load` order.
    pub fn load_saved_data(&mut self, data: &NbtCompound) {
        let compounds = |key: &str| {
            data.get_list(key)
                .unwrap_or_default()
                .iter()
                .filter_map(NbtTag::extract_compound)
                .collect::<Vec<_>>()
        };
        for objective in compounds("Objectives") {
            let Some(name) = objective.get_string("Name") else {
                continue;
            };
            let render_type = match objective.get_string("RenderType") {
                Some("hearts") => RenderType::Hearts,
                _ => RenderType::Integer,
            };
            let mut loaded = ScoreboardObjective::new(
                name,
                objective.get("DisplayName").map_or_else(
                    || TextComponent::text(name.to_string()),
                    TextComponent::from_nbt,
                ),
                render_type,
                objective
                    .get_compound("format")
                    .and_then(number_format_from_nbt),
                objective.get_string("CriteriaName").unwrap_or("dummy"),
            );
            loaded.display_auto_update = objective.get_bool("display_auto_update").unwrap_or(false);
            self.objectives.insert(name.to_string(), loaded);
        }
        for score in compounds("PlayerScores") {
            let (Some(owner), Some(objective)) =
                (score.get_string("Name"), score.get_string("Objective"))
            else {
                continue;
            };
            if !self.objectives.contains_key(objective) {
                warn!("Unknown objective {objective} for name {owner}, ignoring");
                continue;
            }
            let entry = ScoreboardScore {
                entity_name: owner.to_string(),
                objective_name: objective.to_string(),
                value: VarInt(score.get_int("Score").unwrap_or(0)),
                display_name: score.get("display").map(TextComponent::from_nbt),
                number_format: score
                    .get_compound("format")
                    .and_then(number_format_from_nbt),
                locked: score.get_bool("Locked").unwrap_or(false),
            };
            self.scores
                .entry(entry.objective_name.clone())
                .or_default()
                .insert(entry.entity_name.clone(), entry);
        }
        if let Some(slots) = data.get_compound("DisplaySlots") {
            for slot in ALL_DISPLAY_SLOTS {
                if let Some(name) = slots.get_string(display_slot_name(slot))
                    && self.objectives.contains_key(name)
                {
                    self.display_slots.insert(slot, name.to_string());
                }
            }
        }
        for team in compounds("Teams") {
            if let Some(team) = Team::load(team) {
                for player in &team.players {
                    for other in self.teams.values_mut() {
                        other.players.retain(|p| p != player);
                    }
                }
                self.teams.insert(team.name.clone(), team);
            }
        }
    }

    /// Vanilla's `ScoreboardSaveData.Packed` of this scoreboard.
    #[must_use]
    pub fn to_saved_data(&self) -> NbtCompound {
        let objectives = self
            .objectives
            .values()
            .map(|objective| {
                let mut nbt = NbtCompound::new();
                nbt.put_string("Name", objective.name.clone());
                if objective.criterion != "dummy" {
                    nbt.put_string("CriteriaName", objective.criterion.clone());
                }
                nbt.put("DisplayName", objective.display_name.0.to_nbt_tag());
                if objective.render_type == RenderType::Hearts {
                    nbt.put_string("RenderType", "hearts".to_string());
                }
                if objective.display_auto_update {
                    nbt.put_bool("display_auto_update", true);
                }
                if let Some(format) = &objective.number_format {
                    nbt.put_compound("format", number_format_to_nbt(format));
                }
                NbtTag::Compound(nbt)
            })
            .collect();
        let scores = self
            .scores
            .values()
            .flat_map(FxHashMap::values)
            .map(|score| {
                let mut nbt = NbtCompound::new();
                nbt.put_string("Name", score.entity_name.clone());
                nbt.put_string("Objective", score.objective_name.clone());
                if score.value.0 != 0 {
                    nbt.put_int("Score", score.value.0);
                }
                if score.locked {
                    nbt.put_bool("Locked", true);
                }
                if let Some(display) = &score.display_name {
                    nbt.put("display", display.0.to_nbt_tag());
                }
                if let Some(format) = &score.number_format {
                    nbt.put_compound("format", number_format_to_nbt(format));
                }
                NbtTag::Compound(nbt)
            })
            .collect();
        let mut slots = NbtCompound::new();
        for slot in ALL_DISPLAY_SLOTS {
            if let Some(name) = self.display_slots.get(&slot) {
                slots.put_string(display_slot_name(slot), name.clone());
            }
        }
        let mut data = NbtCompound::new();
        data.put_list("Objectives", objectives);
        data.put_list("PlayerScores", scores);
        data.put_compound("DisplaySlots", slots);
        data.put_list(
            "Teams",
            self.teams
                .values()
                .map(|team| NbtTag::Compound(team.pack()))
                .collect(),
        );
        data
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BedrockSortOrder {
    Ascending,
    Descending,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BedrockDisplaySlot {
    PlayerList,
    Sidebar,
    BelowName,
}

impl BedrockDisplaySlot {
    #[must_use]
    pub const fn to_str(&self) -> &'static str {
        match self {
            Self::PlayerList => "list",
            Self::Sidebar => "sidebar",
            Self::BelowName => "belowname",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BedrockObjective {
    pub name: String,
    pub display_name: String,
    pub sort_order: BedrockSortOrder,
}

#[derive(Clone, Debug, Default)]
pub struct BedrockScoreboard {
    pub objectives: FxHashMap<String, BedrockObjective>,
    pub display_slots: FxHashMap<BedrockDisplaySlot, String>,
    pub scores: FxHashMap<(String, String), i32>,
}

impl BedrockScoreboard {
    pub fn add_objective(&mut self, player: &Player, objective: BedrockObjective) {
        let be_update = BSetDisplayObjective {
            display_slot_name: "sidebar".to_string(),
            objective_name: objective.name.clone(),
            objective_display_name: objective.display_name.clone(),
            criteria_name: "dummy".to_string(),
            sort_order: VarInt(match objective.sort_order {
                BedrockSortOrder::Ascending => 0,
                BedrockSortOrder::Descending => 1,
            }),
        };
        player.try_enqueue_packet_editioned(
            &pumpkin_protocol::java::client::play::CUpdateObjectives::new(
                objective.name.clone(),
                pumpkin_protocol::java::client::play::Mode::Add,
                TextComponent::text(objective.display_name.clone()),
                pumpkin_protocol::java::client::play::RenderType::Integer,
                None,
            ),
            &be_update,
        );

        self.objectives.insert(objective.name.clone(), objective);
    }

    pub fn update_objective(&mut self, player: &Player, objective: BedrockObjective) {
        let be_update = BSetDisplayObjective {
            display_slot_name: "sidebar".to_string(),
            objective_name: objective.name.clone(),
            objective_display_name: objective.display_name.clone(),
            criteria_name: "dummy".to_string(),
            sort_order: VarInt(match objective.sort_order {
                BedrockSortOrder::Ascending => 0,
                BedrockSortOrder::Descending => 1,
            }),
        };
        player.try_enqueue_packet_editioned(
            &pumpkin_protocol::java::client::play::CUpdateObjectives::new(
                objective.name.clone(),
                pumpkin_protocol::java::client::play::Mode::Update,
                TextComponent::text(objective.display_name.clone()),
                pumpkin_protocol::java::client::play::RenderType::Integer,
                None,
            ),
            &be_update,
        );

        self.objectives.insert(objective.name.clone(), objective);
    }

    pub fn remove_objective(&mut self, player: &Player, name: &str) {
        let be_remove = BRemoveObjective {
            objective_name: name.to_string(),
        };
        player.try_enqueue_packet_editioned(
            &pumpkin_protocol::java::client::play::CUpdateObjectives::new(
                name.to_string(),
                pumpkin_protocol::java::client::play::Mode::Remove,
                TextComponent::text(""),
                pumpkin_protocol::java::client::play::RenderType::Integer,
                None,
            ),
            &be_remove,
        );
        self.objectives.remove(name);
        self.display_slots.retain(|_, v| v != name);
        self.scores.retain(|(_, obj), _| obj != name);
    }

    pub fn set_display_objective(
        &mut self,
        player: &Player,
        slot: BedrockDisplaySlot,
        objective_name: Option<&str>,
    ) {
        let obj_name_str = objective_name.unwrap_or("");
        let display_name = objective_name
            .and_then(|name| self.objectives.get(name))
            .map_or_else(|| obj_name_str.to_string(), |o| o.display_name.clone());

        let be_display = BSetDisplayObjective {
            display_slot_name: slot.to_str().to_string(),
            objective_name: obj_name_str.to_string(),
            objective_display_name: display_name,
            criteria_name: "dummy".to_string(),
            sort_order: VarInt(0),
        };
        player.try_enqueue_packet_editioned(
            &pumpkin_protocol::java::client::play::CDisplayObjective::new(
                match slot {
                    BedrockDisplaySlot::PlayerList => ScoreboardDisplaySlot::List,
                    BedrockDisplaySlot::Sidebar => ScoreboardDisplaySlot::Sidebar,
                    BedrockDisplaySlot::BelowName => ScoreboardDisplaySlot::BelowName,
                },
                obj_name_str.to_string(),
            ),
            &be_display,
        );

        if let Some(name) = objective_name {
            self.display_slots.insert(slot, name.to_string());
        } else {
            self.display_slots.remove(&slot);
        }
    }

    pub fn clear_display_objective(&mut self, player: &Player, slot: BedrockDisplaySlot) {
        self.set_display_objective(player, slot, None);
    }

    pub fn update_score(
        &mut self,
        player: &Player,
        entity_name: &str,
        objective_name: &str,
        value: i32,
    ) {
        let score = ScoreboardScore::new(
            entity_name.to_string(),
            objective_name.to_string(),
            VarInt(value),
            None,
            None,
        );
        let be_score = BSetScore {
            action: VarInt(0),
            entries: vec![],
        };
        player.try_enqueue_packet_editioned(
            &pumpkin_protocol::java::client::play::CUpdateScore::new(
                score.entity_name.clone(),
                score.objective_name.clone(),
                score.value,
                score.display_name.clone(),
                score.number_format,
            ),
            &be_score,
        );

        self.scores
            .insert((entity_name.to_string(), objective_name.to_string()), value);
    }

    pub fn add_score(
        &mut self,
        player: &Player,
        entity_name: impl Into<String>,
        objective_name: impl Into<String>,
        delta: i32,
    ) -> i32 {
        let entity_s = entity_name.into();
        let obj_s = objective_name.into();
        let current = self
            .scores
            .get(&(entity_s.clone(), obj_s.clone()))
            .copied()
            .unwrap_or(0);
        let new_val = current + delta;
        self.update_score(player, &entity_s, &obj_s, new_val);
        new_val
    }

    pub fn remove_score(&mut self, player: &Player, entity_name: &str, objective_name: &str) {
        let be_score = BSetScore {
            action: VarInt(1),
            entries: vec![],
        };
        player.try_enqueue_packet_editioned(
            &pumpkin_protocol::java::client::play::CResetScore::new(
                entity_name.to_string(),
                Some(objective_name.to_string()),
            ),
            &be_score,
        );
        self.scores
            .remove(&(entity_name.to_string(), objective_name.to_string()));
    }

    pub fn reset_scores_for_entity(&mut self, player: &Player, entity_name: &str) {
        let be_score = BSetScore {
            action: VarInt(1),
            entries: vec![],
        };
        player.try_enqueue_packet_editioned(
            &pumpkin_protocol::java::client::play::CResetScore::new(entity_name.to_string(), None),
            &be_score,
        );
        self.scores.retain(|(entity, _), _| entity != entity_name);
    }

    pub fn send_to_player(&self, player: &Player) {
        for objective in self.objectives.values() {
            let be_update = BSetDisplayObjective {
                display_slot_name: "sidebar".to_string(),
                objective_name: objective.name.clone(),
                objective_display_name: objective.display_name.clone(),
                criteria_name: "dummy".to_string(),
                sort_order: VarInt(match objective.sort_order {
                    BedrockSortOrder::Ascending => 0,
                    BedrockSortOrder::Descending => 1,
                }),
            };
            player.try_enqueue_packet_editioned(
                &pumpkin_protocol::java::client::play::CUpdateObjectives::new(
                    objective.name.clone(),
                    pumpkin_protocol::java::client::play::Mode::Add,
                    TextComponent::text(objective.display_name.clone()),
                    pumpkin_protocol::java::client::play::RenderType::Integer,
                    None,
                ),
                &be_update,
            );
        }

        for (slot, objective_name) in &self.display_slots {
            let display_name = self
                .objectives
                .get(objective_name)
                .map_or_else(|| objective_name.clone(), |o| o.display_name.clone());
            let be_display = BSetDisplayObjective {
                display_slot_name: slot.to_str().to_string(),
                objective_name: objective_name.clone(),
                objective_display_name: display_name,
                criteria_name: "dummy".to_string(),
                sort_order: VarInt(0),
            };
            player.try_enqueue_packet_editioned(
                &pumpkin_protocol::java::client::play::CDisplayObjective::new(
                    match slot {
                        BedrockDisplaySlot::PlayerList => ScoreboardDisplaySlot::List,
                        BedrockDisplaySlot::Sidebar => ScoreboardDisplaySlot::Sidebar,
                        BedrockDisplaySlot::BelowName => ScoreboardDisplaySlot::BelowName,
                    },
                    objective_name.clone(),
                ),
                &be_display,
            );
        }

        for ((entity_name, objective_name), value) in &self.scores {
            let score = ScoreboardScore::new(
                entity_name.clone(),
                objective_name.clone(),
                VarInt(*value),
                None,
                None,
            );
            let be_score = BSetScore {
                action: VarInt(0),
                entries: vec![],
            };
            player.try_enqueue_packet_editioned(
                &pumpkin_protocol::java::client::play::CUpdateScore::new(
                    score.entity_name,
                    score.objective_name,
                    score.value,
                    score.display_name,
                    score.number_format,
                ),
                &be_score,
            );
        }
    }
}

#[derive(Default, Debug)]
pub struct ScoreboardBuilder {
    scoreboard: Scoreboard,
}

impl ScoreboardBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn objective(mut self, name: impl Into<String>, display_name: TextComponent) -> Self {
        let obj = ScoreboardObjective::new(
            name.into(),
            display_name,
            RenderType::Integer,
            None,
            "dummy",
        );
        self.scoreboard.objectives.insert(obj.name.clone(), obj);
        self
    }

    #[must_use]
    pub fn objective_with_render(
        mut self,
        name: impl Into<String>,
        display_name: TextComponent,
        render_type: RenderType,
    ) -> Self {
        let obj = ScoreboardObjective::new(name.into(), display_name, render_type, None, "dummy");
        self.scoreboard.objectives.insert(obj.name.clone(), obj);
        self
    }

    #[must_use]
    pub fn display_slot(
        mut self,
        slot: ScoreboardDisplaySlot,
        objective_name: impl Into<String>,
    ) -> Self {
        self.scoreboard
            .display_slots
            .insert(slot, objective_name.into());
        self
    }

    #[must_use]
    pub fn score(
        mut self,
        entity_name: impl Into<String>,
        objective_name: impl Into<String>,
        value: i32,
    ) -> Self {
        let entity_s = entity_name.into();
        let obj_s = objective_name.into();
        let score =
            ScoreboardScore::new(entity_s.clone(), obj_s.clone(), VarInt(value), None, None);
        self.scoreboard
            .scores
            .entry(obj_s)
            .or_default()
            .insert(entity_s, score);
        self
    }

    #[must_use]
    pub fn team(mut self, team: Team) -> Self {
        self.scoreboard.teams.insert(team.name.clone(), team);
        self
    }

    #[must_use]
    pub fn build(self) -> Scoreboard {
        self.scoreboard
    }
}

#[derive(Default, Debug)]
pub struct BedrockScoreboardBuilder {
    scoreboard: BedrockScoreboard,
}

impl BedrockScoreboardBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn objective(
        mut self,
        name: impl Into<String>,
        display_name: impl Into<String>,
        sort_order: BedrockSortOrder,
    ) -> Self {
        let name = name.into();
        let obj = BedrockObjective {
            name: name.clone(),
            display_name: display_name.into(),
            sort_order,
        };
        self.scoreboard.objectives.insert(name, obj);
        self
    }

    #[must_use]
    pub fn display_slot(
        mut self,
        slot: BedrockDisplaySlot,
        objective_name: impl Into<String>,
    ) -> Self {
        self.scoreboard
            .display_slots
            .insert(slot, objective_name.into());
        self
    }

    #[must_use]
    pub fn score(
        mut self,
        entity_name: impl Into<String>,
        objective_name: impl Into<String>,
        value: i32,
    ) -> Self {
        self.scoreboard
            .scores
            .insert((entity_name.into(), objective_name.into()), value);
        self
    }

    #[must_use]
    pub fn build(self) -> BedrockScoreboard {
        self.scoreboard
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compound(entries: Vec<(&str, NbtTag)>) -> NbtCompound {
        let mut nbt = NbtCompound::new();
        for (key, tag) in entries {
            nbt.put(key, tag);
        }
        nbt
    }

    fn string(s: &str) -> NbtTag {
        NbtTag::String(s.into())
    }

    /// Records the packet types a scoreboard sends.
    #[derive(Default)]
    struct Recorder(std::sync::Mutex<Vec<&'static str>>);

    impl Recorder {
        fn record<J>(&self) {
            let name = std::any::type_name::<J>()
                .rsplit("::")
                .next()
                .unwrap_or_default();
            self.0.lock().unwrap().push(name);
        }

        fn take(&self) -> Vec<&'static str> {
            std::mem::take(&mut *self.0.lock().unwrap())
        }
    }

    impl ScoreboardTarget for Recorder {
        fn send_editioned<J: ClientPacket + Sync, B: BClientPacket + Sync>(&self, _: &J, _: &B) {
            self.record::<J>();
        }

        fn send_je<J: ClientPacket + Sync>(&self, _: &J) {
            self.record::<J>();
        }
    }

    /// Vanilla `ServerScoreboard` only sends objectives while a display slot shows them.
    #[test]
    fn objectives_are_sent_only_while_displayed() {
        let target = Recorder::default();
        let mut scoreboard = Scoreboard::new();
        let objective = |name: &str| {
            ScoreboardObjective::new(
                name,
                TextComponent::text(name.to_string()),
                RenderType::Integer,
                None,
                "dummy",
            )
        };
        scoreboard.add_objective(&target, objective("a"));
        scoreboard.add_objective(&target, objective("b"));
        scoreboard.set_score_value(&target, "Steve", "a", 1);
        assert!(target.take().is_empty());

        scoreboard.set_display_objective(&target, ScoreboardDisplaySlot::Sidebar, Some("a"));
        assert_eq!(
            target.take(),
            ["CUpdateObjectives", "CDisplayObjective", "CUpdateScore"]
        );
        // Unchanged scores are not resent.
        scoreboard.set_score_value(&target, "Steve", "a", 1);
        scoreboard.set_score_value(&target, "Steve", "b", 1);
        assert!(target.take().is_empty());

        // Replacing the only slot showing `a` stops tracking it and starts tracking `b`.
        scoreboard.set_display_objective(&target, ScoreboardDisplaySlot::Sidebar, Some("b"));
        assert_eq!(
            target.take(),
            [
                "CUpdateObjectives",
                "CUpdateObjectives",
                "CDisplayObjective",
                "CUpdateScore"
            ]
        );

        scoreboard.remove_objective(&target, "b");
        assert_eq!(target.take(), ["CUpdateObjectives"]);
        assert!(
            scoreboard
                .get_display_objective(ScoreboardDisplaySlot::Sidebar)
                .is_none()
        );
    }

    /// Vanilla `Scoreboard.addPlayerToTeam` moves a player out of its old team.
    #[test]
    fn joining_a_team_leaves_the_old_one() {
        let team = |name: &str| Team {
            name: name.to_string(),
            display_name: TextComponent::text(name.to_string()),
            options: 0,
            nametag_visibility: NameTagVisibility::Always,
            death_message_visibility: NameTagVisibility::Always,
            collision_rule: CollisionRule::Always,
            color: None,
            player_prefix: TextComponent::empty(),
            player_suffix: TextComponent::empty(),
            players: vec![],
        };
        let mut scoreboard = Scoreboard::new();
        scoreboard.add_team(&NoTarget, team("red"));
        scoreboard.add_team(&NoTarget, team("blue"));
        assert!(scoreboard.add_player_to_team(&NoTarget, "red", "Steve".to_string()));
        assert!(!scoreboard.add_player_to_team(&NoTarget, "red", "Steve".to_string()));
        assert!(scoreboard.add_player_to_team(&NoTarget, "blue", "Steve".to_string()));
        assert!(scoreboard.get_team("red").unwrap().players.is_empty());
        assert_eq!(scoreboard.get_entity_team("Steve").unwrap().name, "blue");
    }

    #[test]
    fn builder_scores_are_found_by_holder_and_objective() {
        let scoreboard = ScoreboardBuilder::new()
            .objective("kills", TextComponent::text("Kills"))
            .score("Steve", "kills", 3)
            .build();
        assert_eq!(scoreboard.get_score_value("Steve", "kills"), Some(3));
    }

    #[test]
    fn white_and_uncoloured_teams_stay_distinct() {
        let team = |name: &str, color: Option<&str>| {
            let mut entries = vec![("Name", string(name)), ("DisplayName", string(name))];
            if let Some(color) = color {
                entries.push(("TeamColor", string(color)));
            }
            NbtTag::Compound(compound(entries))
        };
        let data = compound(vec![
            ("Objectives", NbtTag::List(vec![])),
            ("PlayerScores", NbtTag::List(vec![])),
            ("DisplaySlots", NbtTag::Compound(NbtCompound::new())),
            (
                "Teams",
                NbtTag::List(vec![team("white", Some("white")), team("plain", None)]),
            ),
        ]);
        let mut scoreboard = Scoreboard::new();
        scoreboard.load_saved_data(&data);
        assert_eq!(
            scoreboard.get_team("white").unwrap().color,
            Some(NamedColor::White)
        );
        assert_eq!(scoreboard.get_team("plain").unwrap().color, None);
        let saved = scoreboard.to_saved_data();
        let teams = saved.get_list("Teams").unwrap();
        let color_of = |name: &str| {
            teams
                .iter()
                .filter_map(NbtTag::extract_compound)
                .find(|t| t.get_string("Name") == Some(name))
                .and_then(|t| t.get_string("TeamColor").map(str::to_string))
        };
        assert_eq!(color_of("white").as_deref(), Some("white"));
        assert_eq!(color_of("plain"), None);
    }

    /// A `scoreboard.dat` written by a 26.3 server loads and saves back unchanged.
    #[test]
    fn vanilla_scoreboard_data_round_trips() {
        let objective = compound(vec![
            ("Name", string("custom")),
            ("CriteriaName", string("health")),
            (
                "DisplayName",
                NbtTag::Compound(compound(vec![
                    ("text", string("Custom")),
                    ("color", string("gold")),
                ])),
            ),
            ("RenderType", string("hearts")),
            ("display_auto_update", NbtTag::Byte(1)),
            (
                "format",
                NbtTag::Compound(compound(vec![
                    ("color", string("red")),
                    ("type", string("minecraft:styled")),
                ])),
            ),
        ]);
        let score = compound(vec![
            ("Name", string("#fake")),
            ("Objective", string("custom")),
            ("Score", NbtTag::Int(7)),
            ("Locked", NbtTag::Byte(1)),
            ("display", string("Fake")),
        ]);
        let team = compound(vec![
            ("Name", string("red")),
            ("DisplayName", string("Red Team")),
            ("TeamColor", string("red")),
            ("AllowFriendlyFire", NbtTag::Byte(0)),
            ("SeeFriendlyInvisibles", NbtTag::Byte(0)),
            ("MemberNamePrefix", string("[R] ")),
            ("MemberNameSuffix", string(" !")),
            ("NameTagVisibility", string("hideForOtherTeams")),
            ("DeathMessageVisibility", string("never")),
            ("CollisionRule", string("pushOwnTeam")),
            (
                "Players",
                NbtTag::List(vec![string("#fake"), string("Bot")]),
            ),
        ]);
        let data = compound(vec![
            (
                "Objectives",
                NbtTag::List(vec![NbtTag::Compound(objective)]),
            ),
            ("PlayerScores", NbtTag::List(vec![NbtTag::Compound(score)])),
            (
                "DisplaySlots",
                NbtTag::Compound(compound(vec![("below_name", string("custom"))])),
            ),
            ("Teams", NbtTag::List(vec![NbtTag::Compound(team)])),
        ]);

        let mut scoreboard = Scoreboard::new();
        scoreboard.load_saved_data(&data);
        assert_eq!(scoreboard.to_saved_data(), data);
    }
}
