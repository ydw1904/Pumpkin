use serde::{Deserialize, Serialize};

/// Lighting engine calculation mode.
#[derive(Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum LightingEngineConfig {
    /// Default Vanilla lighting propagation.
    #[default]
    Default,
    /// Full skylight everywhere (no shadows).
    Full,
    /// Completely dark lighting everywhere (zero light).
    Dark,
}

impl LightingEngineConfig {
    /// The light level every block has in this mode, or `None` when light is propagated.
    #[must_use]
    pub const fn fixed_level(self) -> Option<u8> {
        match self {
            Self::Default => None,
            Self::Full => Some(15),
            Self::Dark => Some(0),
        }
    }
}
