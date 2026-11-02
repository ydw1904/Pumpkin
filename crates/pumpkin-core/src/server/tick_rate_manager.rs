use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, Ordering};
use std::time::Instant;

use crossbeam::atomic::AtomicCell;
use pumpkin_data::translation;
use pumpkin_protocol::java::client::play::{CTickingState, CTickingStep};
use pumpkin_util::text::TextComponent;

use crate::command::CommandSender;
use crate::entity::player::Player;
use crate::server::Server;
const NANOSECONDS_PER_SECOND: i64 = 1_000_000_000;
const NANOSECONDS_PER_MILLISECOND: f64 = 1_000_000.0;
const MILLISECONDS_PER_SECOND: f64 = 1000.0;
/// Vanilla `TickRateManager.MIN_TICKRATE`.
const MIN_TICKRATE: f32 = 1.0;
/// Vanilla `TickCommand.MAX_TICKRATE`. Without this, `nanoseconds_per_tick` could truncates to 0.
const MAX_TICKRATE: f32 = 10000.0;

/// NaN falls back to the minimum.
const fn clamp_tickrate(rate: f32) -> f32 {
    if rate.is_nan() {
        return MIN_TICKRATE;
    }
    rate.clamp(MIN_TICKRATE, MAX_TICKRATE)
}

fn nanoseconds_for_rate(rate: f32) -> i64 {
    (NANOSECONDS_PER_SECOND as f64 / f64::from(rate)) as i64
}

pub struct ServerTickRateManager {
    tickrate: AtomicCell<f32>,
    nanoseconds_per_tick: AtomicI64,
    frozen_ticks_to_run: AtomicI32,
    run_game_elements: AtomicBool,
    is_frozen: AtomicBool,

    // Sprinting state
    remaining_sprint_ticks: AtomicI64,
    sprint_tick_start_time: AtomicCell<Instant>,
    sprint_time_spend: AtomicI64,
    scheduled_current_sprint_ticks: AtomicI64,
    previous_is_frozen: AtomicBool,
}

impl ServerTickRateManager {
    #[must_use]
    pub fn new(tickrate: f32) -> Self {
        let tickrate = clamp_tickrate(tickrate);
        Self {
            tickrate: AtomicCell::new(tickrate),
            nanoseconds_per_tick: AtomicI64::new(nanoseconds_for_rate(tickrate)),
            frozen_ticks_to_run: AtomicI32::new(0),
            run_game_elements: AtomicBool::new(true),
            is_frozen: AtomicBool::new(false),
            remaining_sprint_ticks: AtomicI64::new(0),
            sprint_tick_start_time: AtomicCell::new(Instant::now()),
            sprint_time_spend: AtomicI64::new(0),
            scheduled_current_sprint_ticks: AtomicI64::new(0),
            previous_is_frozen: AtomicBool::new(false),
        }
    }
}

impl ServerTickRateManager {
    pub fn tick(&self) {
        let frozen_ticks = self.frozen_ticks_to_run.load(Ordering::Relaxed);
        let run_game = !self.is_frozen.load(Ordering::Relaxed) || frozen_ticks > 0;
        self.run_game_elements.store(run_game, Ordering::Relaxed);

        if frozen_ticks > 0 {
            self.frozen_ticks_to_run.fetch_sub(1, Ordering::Relaxed);
        }
    }

    // Getters
    pub fn tickrate(&self) -> f32 {
        self.tickrate.load()
    }

    pub fn nanoseconds_per_tick(&self) -> i64 {
        self.nanoseconds_per_tick.load(Ordering::Relaxed)
    }

    pub fn is_frozen(&self) -> bool {
        self.is_frozen.load(Ordering::Relaxed)
    }

    pub fn runs_normally(&self) -> bool {
        self.run_game_elements.load(Ordering::Relaxed)
    }

    pub fn is_sprinting(&self) -> bool {
        self.remaining_sprint_ticks.load(Ordering::Relaxed) > 0
    }

    pub fn is_stepping_forward(&self) -> bool {
        self.frozen_ticks_to_run.load(Ordering::Relaxed) > 0
    }

    pub fn set_tick_rate(&self, server: &Server, rate: f32) {
        let rate = clamp_tickrate(rate);
        self.tickrate.store(rate);
        self.nanoseconds_per_tick
            .store(nanoseconds_for_rate(rate), Ordering::Relaxed);
        // server.on_tick_rate_changed(); // Might need this hook if autosave interval depends on it
        self.update_state_to_clients(server);
    }

    pub fn set_frozen(&self, server: &Server, frozen: bool) {
        if frozen {
            pumpkin_util::random::gameplay::reset();
        }
        self.is_frozen.store(frozen, Ordering::Relaxed);
        self.update_state_to_clients(server);
    }

    pub fn step_game_if_paused(&self, server: &Server, ticks: i32) -> bool {
        if !self.is_frozen() {
            return false;
        }
        self.frozen_ticks_to_run.store(ticks, Ordering::Relaxed);
        self.update_step_ticks(server);
        true
    }

    pub fn stop_stepping(&self, server: &Server) -> bool {
        if self.is_stepping_forward() {
            self.frozen_ticks_to_run.store(0, Ordering::Relaxed);
            self.update_step_ticks(server);
            true
        } else {
            false
        }
    }

    pub fn request_game_to_sprint(&self, server: &Server, ticks: i64) -> bool {
        let was_sprinting = self.is_sprinting();
        self.sprint_time_spend.store(0, Ordering::Relaxed);
        self.scheduled_current_sprint_ticks
            .store(ticks, Ordering::Relaxed);
        self.remaining_sprint_ticks.store(ticks, Ordering::Relaxed);
        self.previous_is_frozen
            .store(self.is_frozen(), Ordering::Relaxed);
        self.set_frozen(server, false);
        was_sprinting
    }

    pub fn stop_sprinting(&self, server: &Arc<Server>) -> bool {
        if self.is_sprinting() {
            self.finish_tick_sprint(server);
            true
        } else {
            false
        }
    }

    /// Records the start time of a sprint tick's workload.
    pub fn start_sprint_tick_work(&self) {
        self.sprint_tick_start_time.store(Instant::now());
    }

    /// Records the time spent on the sprint tick's work and decrements the remaining sprint ticks.
    /// Returns `true` if the sprint has just finished.
    pub fn end_sprint_tick_work(&self) -> bool {
        let spent = Instant::now().duration_since(self.sprint_tick_start_time.load());
        self.sprint_time_spend
            .fetch_add(spent.as_nanos() as i64, Ordering::Relaxed);

        // fetch_sub returns the *previous* value. If it was 1, it is now 0, and the sprint is over.
        self.remaining_sprint_ticks.fetch_sub(1, Ordering::Relaxed) == 1
    }

    /// Vanilla `ServerTickRateManager.finishTickSprint`.
    pub fn finish_tick_sprint(&self, server: &Arc<Server>) {
        let completed_ticks = self.scheduled_current_sprint_ticks.load(Ordering::Relaxed)
            - self.remaining_sprint_ticks.load(Ordering::Relaxed);
        let milliseconds_to_complete = (self.sprint_time_spend.load(Ordering::Relaxed) as f64)
            .max(1.0)
            / NANOSECONDS_PER_MILLISECOND;
        let ticks_per_second =
            (MILLISECONDS_PER_SECOND * completed_ticks as f64 / milliseconds_to_complete) as i32;
        let milliseconds_per_tick = if completed_ticks == 0 {
            f64::from(self.nanoseconds_per_tick() as f32 / NANOSECONDS_PER_MILLISECOND as f32)
        } else {
            milliseconds_to_complete / completed_ticks as f64
        };
        self.scheduled_current_sprint_ticks
            .store(0, Ordering::Relaxed);
        self.sprint_time_spend.store(0, Ordering::Relaxed);
        CommandSender::Console.into_source(server).send_feedback(
            TextComponent::translate_cross(
                translation::java::COMMANDS_TICK_SPRINT_REPORT,
                translation::java::COMMANDS_TICK_SPRINT_REPORT,
                [
                    TextComponent::text(ticks_per_second.to_string()),
                    TextComponent::text(format!("{milliseconds_per_tick:.2}")),
                ],
            ),
            true,
        );
        self.remaining_sprint_ticks.store(0, Ordering::Relaxed);
        self.set_frozen(server, self.previous_is_frozen.load(Ordering::Relaxed));
        // server.on_tick_rate_changed();
    }

    fn update_state_to_clients(&self, server: &Server) {
        server.broadcast_packet_all(&CTickingState::new(
            self.tickrate.load(),
            self.is_frozen.load(Ordering::Relaxed),
        ));
    }

    fn update_step_ticks(&self, server: &Server) {
        server.broadcast_packet_all(&CTickingStep::new(
            self.frozen_ticks_to_run.load(Ordering::Relaxed).into(),
        ));
    }
    pub async fn update_joining_player(&self, player: &Player) {
        if player.client.java_version() >= pumpkin_util::version::JavaMinecraftVersion::V_1_20_3 {
            player
                .send_client_packet(&CTickingState::new(self.tickrate(), self.is_frozen()))
                .await;
            player
                .send_client_packet(&CTickingStep::new(
                    self.frozen_ticks_to_run.load(Ordering::Relaxed).into(),
                ))
                .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_TICKRATE, MIN_TICKRATE, clamp_tickrate, nanoseconds_for_rate};

    #[test]
    fn clamp_tickrate_rejects_nan() {
        assert_eq!(clamp_tickrate(f32::NAN), MIN_TICKRATE);
        // NaN would cast to a 0ns tick.
        assert!(nanoseconds_for_rate(clamp_tickrate(f32::NAN)) > 0);
    }

    #[test]
    fn clamp_tickrate_bounds_finite_rates() {
        assert_eq!(clamp_tickrate(0.0), MIN_TICKRATE);
        assert_eq!(clamp_tickrate(-5.0), MIN_TICKRATE);
        assert_eq!(clamp_tickrate(f32::NEG_INFINITY), MIN_TICKRATE);
        assert_eq!(clamp_tickrate(f32::INFINITY), MAX_TICKRATE);
        assert_eq!(clamp_tickrate(20.0), 20.0);
    }
}
