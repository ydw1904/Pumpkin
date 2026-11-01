use std::sync::Arc;

use crate::entity::player::Player;

/// Vanilla `SleepStatus`: how many players count towards skipping the night and how many sleep.
#[derive(Default)]
pub struct SleepStatus {
    active_players: i32,
    sleeping_players: i32,
}

impl SleepStatus {
    #[must_use]
    pub fn are_enough_sleeping(&self, sleep_percentage_needed: i64) -> bool {
        self.sleeping_players >= self.sleepers_needed(sleep_percentage_needed)
    }

    #[must_use]
    pub fn are_enough_deep_sleeping(
        &self,
        sleep_percentage_needed: i64,
        players: &[Arc<Player>],
    ) -> bool {
        let deep_sleepers = players
            .iter()
            .filter(|player| player.is_sleeping_long_enough())
            .count();
        deep_sleepers as i32 >= self.sleepers_needed(sleep_percentage_needed)
    }

    #[must_use]
    pub fn sleepers_needed(&self, sleep_percentage_needed: i64) -> i32 {
        let needed =
            (i64::from(self.active_players).saturating_mul(sleep_percentage_needed) as f32 / 100.0)
                .ceil();
        (needed as i32).max(1)
    }

    pub const fn remove_all_sleepers(&mut self) {
        self.sleeping_players = 0;
    }

    #[must_use]
    pub const fn amount_sleeping(&self) -> i32 {
        self.sleeping_players
    }

    /// Recounts the players and returns whether players are or were sleeping and a count changed.
    pub fn update(&mut self, players: &[Arc<Player>]) -> bool {
        let old_active = self.active_players;
        let old_sleeping = self.sleeping_players;
        self.active_players = 0;
        self.sleeping_players = 0;
        for player in players {
            if !player.is_spectator() {
                self.active_players += 1;
                if player.sleeping_since.load().is_some() {
                    self.sleeping_players += 1;
                }
            }
        }
        (old_sleeping > 0 || self.sleeping_players > 0)
            && (old_active != self.active_players || old_sleeping != self.sleeping_players)
    }
}
