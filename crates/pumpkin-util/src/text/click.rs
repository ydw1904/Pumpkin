use std::borrow::Cow;

use serde::{Deserialize, Serialize};

/// Action to take on click of the text.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, Eq, Hash)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ClickEvent {
    /// Opens a URL.
    OpenUrl { url: Cow<'static, str> },
    /// Opens a file.
    OpenFile { path: Cow<'static, str> },
    /// Works in signs but only on the root text component.
    RunCommand { command: Cow<'static, str> },
    /// Replaces the contents of the chat box with the text, not necessarily a
    /// command.
    SuggestCommand { command: Cow<'static, str> },
    /// Only usable within written books. Changes the page of the book. Indexing
    /// starts at 1.
    ChangePage { page: u32 },
    /// Copies the given text to the system clipboard.
    CopyToClipboard { value: Cow<'static, str> },
    /// Opens a dialog: a dialog id, or a dialog written inline.
    ShowDialog { dialog: ClickPayload },
    /// Sends `id` and `payload` back to the server as a custom click action.
    Custom {
        id: Cow<'static, str>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        payload: Option<ClickPayload>,
    },
}

/// A dialog or custom payload in a click event, kept as the JSON it was given.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct ClickPayload(pub serde_json::Value);

impl std::hash::Hash for ClickPayload {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.to_string().hash(state);
    }
}

impl ClickPayload {
    /// The payload as NBT, as the network writes it.
    #[must_use]
    pub fn to_nbt(&self) -> pumpkin_nbt::tag::NbtTag {
        super::hover::json_to_nbt(&self.0)
    }
}
