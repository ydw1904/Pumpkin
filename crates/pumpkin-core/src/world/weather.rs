use super::World;
use pumpkin_protocol::java::client::play::{CGameEvent, GameEvent};
use pumpkin_util::random::gameplay::game_random;

// Weather timing constants
const RAIN_DELAY_MIN: i32 = 12_000;
const RAIN_DELAY_MAX: i32 = 180_000;
const RAIN_DURATION_MIN: i32 = 12_000;
const RAIN_DURATION_MAX: i32 = 24_000;
const THUNDER_DELAY_MIN: i32 = 12_000;
const THUNDER_DELAY_MAX: i32 = 180_000;
const THUNDER_DURATION_MIN: i32 = 3_600;
const THUNDER_DURATION_MAX: i32 = 15_600;

const WEATHER_TRANSITION_SPEED: f32 = 0.01;

/// Vanilla `WeatherData` plus the level's rain and thunder levels.
#[derive(Clone)]
pub struct Weather {
    pub clear_weather_time: i32,
    pub raining: bool,
    pub rain_time: i32,
    pub thundering: bool,
    pub thunder_time: i32,

    pub rain_level: f32,
    pub old_rain_level: f32,
    pub thunder_level: f32,
    pub old_thunder_level: f32,
}

impl Default for Weather {
    fn default() -> Self {
        Self::new()
    }
}

impl Weather {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            clear_weather_time: 0,
            raining: false,
            rain_time: 0,
            thundering: false,
            thunder_time: 0,
            rain_level: 0.0,
            old_rain_level: 0.0,
            thunder_level: 0.0,
            old_thunder_level: 0.0,
        }
    }

    /// Vanilla `ServerLevel.prepareWeather`: saved rain or thunder is already at full strength.
    #[must_use]
    pub const fn loaded(
        clear_weather_time: i32,
        rain_time: i32,
        thunder_time: i32,
        raining: bool,
        thundering: bool,
    ) -> Self {
        let mut weather = Self::new();
        weather.clear_weather_time = clear_weather_time;
        weather.rain_time = rain_time;
        weather.thunder_time = thunder_time;
        weather.raining = raining;
        weather.thundering = thundering;
        if raining {
            weather.rain_level = 1.0;
            weather.old_rain_level = 1.0;
            if thundering {
                weather.thunder_level = 1.0;
                weather.old_thunder_level = 1.0;
            }
        }
        weather
    }

    /// Vanilla `Level.isRaining`: the rain has faded in past a fifth.
    #[must_use]
    pub fn is_raining(&self) -> bool {
        self.rain_level > 0.2
    }

    /// Vanilla `Level.getThunderLevel`: thunder only shows as strong as the rain.
    #[must_use]
    pub fn thunder_strength(&self) -> f32 {
        self.thunder_level * self.rain_level
    }

    /// Vanilla `ServerLevel.setWeatherParameters`. The clients hear of it once the levels
    /// move in `tick_weather`.
    pub const fn set_weather_parameters(
        &mut self,
        clear_time: i32,
        rain_time: i32,
        raining: bool,
        thundering: bool,
    ) {
        self.clear_weather_time = clear_time;
        self.rain_time = rain_time;
        self.thunder_time = rain_time;
        self.raining = raining;
        self.thundering = thundering;
    }

    /// Vanilla `ServerLevel.advanceWeatherCycle`.
    pub fn tick_weather(&mut self, world: &World) {
        let was_raining = self.is_raining();
        if world.dimension.has_skylight && !world.dimension.has_ceiling {
            if world.level_info.load().game_rules.advance_weather {
                self.advance_weather_cycle();
            }

            self.old_thunder_level = self.thunder_level;
            self.thunder_level = if self.thundering {
                self.thunder_level + WEATHER_TRANSITION_SPEED
            } else {
                self.thunder_level - WEATHER_TRANSITION_SPEED
            }
            .clamp(0.0, 1.0);

            self.old_rain_level = self.rain_level;
            self.rain_level = if self.raining {
                self.rain_level + WEATHER_TRANSITION_SPEED
            } else {
                self.rain_level - WEATHER_TRANSITION_SPEED
            }
            .clamp(0.0, 1.0);
        }

        if self.old_rain_level != self.rain_level {
            world.broadcast_packet_all(&CGameEvent::new(
                GameEvent::RainLevelChange,
                self.rain_level,
            ));
        }

        if self.old_thunder_level != self.thunder_level {
            world.broadcast_packet_all(&CGameEvent::new(
                GameEvent::ThunderLevelChange,
                self.thunder_level,
            ));
        }

        // Vanilla tells every player on the server, whatever their dimension.
        if was_raining != self.is_raining()
            && let Some(server) = world.server.upgrade()
        {
            let event = if was_raining {
                GameEvent::EndRaining
            } else {
                GameEvent::BeginRaining
            };
            server.broadcast_packet_all(&CGameEvent::new(event, 0.0));
            server.broadcast_packet_all(&CGameEvent::new(
                GameEvent::RainLevelChange,
                self.rain_level,
            ));
            server.broadcast_packet_all(&CGameEvent::new(
                GameEvent::ThunderLevelChange,
                self.thunder_level,
            ));
        }
    }

    fn advance_weather_cycle(&mut self) {
        if self.clear_weather_time > 0 {
            self.clear_weather_time -= 1;
            self.thunder_time = i32::from(!self.thundering);
            self.rain_time = i32::from(!self.raining);
            self.thundering = false;
            self.raining = false;
        } else {
            // Handle thunder timing
            if self.thunder_time > 0 {
                self.thunder_time -= 1;
                if self.thunder_time == 0 {
                    self.thundering = !self.thundering;
                }
            } else if self.thundering {
                self.thunder_time =
                    game_random().random_range(THUNDER_DURATION_MIN..=THUNDER_DURATION_MAX);
            } else {
                self.thunder_time =
                    game_random().random_range(THUNDER_DELAY_MIN..=THUNDER_DELAY_MAX);
            }

            // Handle rain timing
            if self.rain_time > 0 {
                self.rain_time -= 1;
                if self.rain_time == 0 {
                    self.raining = !self.raining;
                }
            } else if self.raining {
                self.rain_time = game_random().random_range(RAIN_DURATION_MIN..=RAIN_DURATION_MAX);
            } else {
                self.rain_time = game_random().random_range(RAIN_DELAY_MIN..=RAIN_DELAY_MAX);
            }
        }
    }

    /// Vanilla `ServerLevel.resetWeatherCycle`.
    pub const fn reset_weather_cycle(&mut self) {
        self.rain_time = 0;
        self.raining = false;
        self.thunder_time = 0;
        self.thundering = false;
    }
}
