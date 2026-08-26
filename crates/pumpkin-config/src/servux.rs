use serde::{Deserialize, Serialize};

/// Configuration for the Servux `servux:hud_metadata` plugin channel.
///
/// Malilib-based clients (`MiniHUD`) open this channel to read world data the
/// vanilla protocol never sends. Vanilla clients never open it, so leaving
/// this enabled has no effect on them.
#[derive(Deserialize, Serialize)]
#[serde(default)]
pub struct ServuxConfig {
    /// Whether requests on the channel are answered at all.
    pub enabled: bool,
    /// Whether the world seed is included in the metadata sent to clients.
    ///
    /// Off by default, matching Servux's own `share_seed` default: the seed
    /// lets clients locate every structure and slime chunk in the world.
    pub share_seed: bool,
}

impl Default for ServuxConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            share_seed: false,
        }
    }
}
