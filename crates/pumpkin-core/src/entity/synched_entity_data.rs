use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use bytes::BufMut;
use pumpkin_data::meta_data_type::MetaDataType;
use pumpkin_data::tracked_data::{TrackedData, TrackedId};
use pumpkin_protocol::java::client::play::{Metadata, MetadataSerializer};
use pumpkin_protocol::ser::WritingError;
use pumpkin_util::version::JavaMinecraftVersion;

pub trait ErasedSerializer: Send + Sync {
    fn write(
        &self,
        index: TrackedId,
        r#type: MetaDataType,
        writer: &mut dyn std::io::Write,
        version: &JavaMinecraftVersion,
    ) -> Result<(), WritingError>;

    fn write_canonical(&self, index: TrackedId, r#type: MetaDataType) -> Vec<u8>;
}

struct SerializerHolder<T> {
    value: T,
}

impl<T: MetadataSerializer + Clone + Send + Sync + 'static> ErasedSerializer
    for SerializerHolder<T>
{
    fn write(
        &self,
        index: TrackedId,
        r#type: MetaDataType,
        writer: &mut dyn std::io::Write,
        version: &JavaMinecraftVersion,
    ) -> Result<(), WritingError> {
        let meta = Metadata::new_raw(index, r#type, &self.value);
        meta.write(writer, version)
    }

    fn write_canonical(&self, index: TrackedId, r#type: MetaDataType) -> Vec<u8> {
        let mut buf = Vec::new();
        let meta = Metadata::new_raw(index, r#type, &self.value);
        let _ = meta.write(&mut buf, &JavaMinecraftVersion::V_26_3);
        buf
    }
}

pub struct DataItem {
    pub tracked: TrackedData,
    pub serializer: Box<dyn ErasedSerializer>,
    pub canonical_bytes: Vec<u8>,
    /// The defined default, `None` while nothing defined one.
    pub default_bytes: Option<Vec<u8>>,
    pub dirty: bool,
    pub is_default: bool,
}

pub struct SynchedEntityData {
    items: Mutex<HashMap<TrackedData, DataItem>>,
    is_dirty: AtomicBool,
}

impl Default for SynchedEntityData {
    fn default() -> Self {
        Self::new()
    }
}

impl SynchedEntityData {
    /// An empty tracker; values are added by [`Self::define`] and [`Self::set`].
    #[must_use]
    pub fn new() -> Self {
        Self {
            items: Mutex::new(HashMap::new()),
            is_dirty: AtomicBool::new(false),
        }
    }

    /// Registers the default the client already assumes for a tracked value, like vanilla
    /// `SynchedEntityData.Builder.define`, so the value is only sent while it differs. A value
    /// set before its default was defined keeps its value.
    pub fn define<T: MetadataSerializer + Clone + Send + Sync + 'static>(
        &self,
        tracked: TrackedData,
        value: T,
    ) {
        let holder = SerializerHolder { value };
        let default_bytes = holder.write_canonical(tracked.id, tracked.r#type);
        let mut items = self
            .items
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(item) = items.get_mut(&tracked) {
            // A subclass default replacing its parent's, like an overridden
            // `getMaxAirSupply`, also replaces a value nothing has set yet.
            if item.is_default && !item.dirty && item.default_bytes.is_some() {
                item.serializer = Box::new(holder);
                item.canonical_bytes.clone_from(&default_bytes);
            }
            item.is_default = item.canonical_bytes == default_bytes;
            // Defined late, a value already set to its default is no change: vanilla defines
            // first, and setting the same value again doesn't mark it dirty.
            if item.default_bytes.is_none() && item.is_default {
                item.dirty = false;
            }
            item.default_bytes = Some(default_bytes);
            return;
        }
        items.insert(
            tracked,
            DataItem {
                tracked,
                serializer: Box::new(holder),
                canonical_bytes: default_bytes.clone(),
                default_bytes: Some(default_bytes),
                dirty: false,
                is_default: true,
            },
        );
    }

    /// Stores a tracked value and marks it dirty, returning whether it differs from
    /// the value that was already stored.
    pub fn set<T: MetadataSerializer + Clone + Send + Sync + 'static>(
        &self,
        tracked: TrackedData,
        value: T,
    ) -> bool {
        let holder = SerializerHolder { value };
        let new_canonical = holder.write_canonical(tracked.id, tracked.r#type);

        let mut items = self
            .items
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(item) = items.get_mut(&tracked) {
            if item.canonical_bytes == new_canonical {
                return false;
            }
            // Vanilla `DataItem.isSetToDefault`: back at its default, a value isn't sent again
            // to clients that start tracking the entity.
            item.is_default = item.default_bytes.as_ref() == Some(&new_canonical);
            item.canonical_bytes = new_canonical;
            item.serializer = Box::new(holder);
            item.dirty = true;
        } else {
            items.insert(
                tracked,
                DataItem {
                    tracked,
                    serializer: Box::new(holder),
                    canonical_bytes: new_canonical,
                    default_bytes: None,
                    dirty: true,
                    is_default: false,
                },
            );
        }
        self.is_dirty.store(true, Ordering::Release);
        true
    }

    /// Like [`Self::set`], but marks the value dirty even when it is unchanged, like vanilla
    /// `SynchedEntityData.set(accessor, value, true)`.
    pub fn set_forced<T: MetadataSerializer + Clone + Send + Sync + 'static>(
        &self,
        tracked: TrackedData,
        value: T,
    ) {
        if self.set(tracked, value) {
            return;
        }
        let mut items = self
            .items
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(item) = items.get_mut(&tracked) {
            item.dirty = true;
            self.is_dirty.store(true, Ordering::Release);
        }
    }

    /// Whether any value changed since the last [`Self::clear_dirty`].
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.is_dirty.load(Ordering::Acquire)
    }

    /// Serializes the values that changed since the last [`Self::clear_dirty`], or
    /// `None` when nothing changed.
    pub fn pack_dirty_for_version(&self, version: &JavaMinecraftVersion) -> Option<Box<[u8]>> {
        if !self.is_dirty.load(Ordering::Acquire) {
            return None;
        }

        let items = self
            .items
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut buf = Vec::new();
        let mut has_any = false;

        for item in in_id_order(&items) {
            if item.dirty {
                let before_len = buf.len();
                if item
                    .serializer
                    .write(item.tracked.id, item.tracked.r#type, &mut buf, version)
                    .is_ok()
                    && buf.len() > before_len
                {
                    has_any = true;
                }
            }
        }

        if !has_any {
            return None;
        }

        buf.put_u8(255);
        Some(buf.into_boxed_slice())
    }

    /// Marks every value as sent.
    pub fn clear_dirty(&self) {
        let mut items = self
            .items
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for item in items.values_mut() {
            item.dirty = false;
        }
        self.is_dirty.store(false, Ordering::Release);
    }

    /// Serializes every value that differs from the default the client assumes, or
    /// `None` when they are all still at their default.
    pub fn get_non_default_values_for_version(
        &self,
        version: &JavaMinecraftVersion,
    ) -> Option<Box<[u8]>> {
        let items = self
            .items
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut buf = Vec::new();
        let mut has_any = false;

        for item in in_id_order(&items) {
            if !item.is_default {
                let before_len = buf.len();
                if item
                    .serializer
                    .write(item.tracked.id, item.tracked.r#type, &mut buf, version)
                    .is_ok()
                    && buf.len() > before_len
                {
                    has_any = true;
                }
            }
        }

        if !has_any {
            return None;
        }

        buf.put_u8(255);
        Some(buf.into_boxed_slice())
    }
}

/// Vanilla packs data items by id, the order the client applies them in.
fn in_id_order(items: &HashMap<TrackedData, DataItem>) -> Vec<&DataItem> {
    let mut ordered: Vec<_> = items.values().collect();
    ordered.sort_unstable_by_key(|item| item.tracked.id.0);
    ordered
}

#[cfg(test)]
mod test {
    use pumpkin_data::tracked_data::player::PLAYER_MODE_CUSTOMISATION;

    use super::*;

    /// Vanilla `packDirty` and `getNonDefaultValues` write the items by id, whatever order
    /// they were set in.
    #[test]
    fn values_are_packed_in_id_order() {
        use pumpkin_data::tracked_data::entity::{
            DATA_CUSTOM_NAME_VISIBLE, DATA_NO_GRAVITY, DATA_SHARED_FLAGS_ID, DATA_SILENT,
        };
        let version = JavaMinecraftVersion::V_26_3;
        let data = SynchedEntityData::new();
        data.set(DATA_NO_GRAVITY, true);
        data.set(DATA_SILENT, true);
        data.set(DATA_CUSTOM_NAME_VISIBLE, true);
        data.set(DATA_SHARED_FLAGS_ID, 1u8);
        // Each item is `id, type, value` with one-byte values here.
        let ids = |buf: Box<[u8]>| buf.chunks(3).map(|item| item[0]).collect::<Vec<_>>();
        let expected = vec![0, 3, 4, 5, 255];
        assert_eq!(
            ids(data.pack_dirty_for_version(&version).unwrap_or_default()),
            expected
        );
        assert_eq!(
            ids(data
                .get_non_default_values_for_version(&version)
                .unwrap_or_default()),
            expected
        );
    }

    /// Metadata ids no longer vary per version: core serializes the current format for
    /// every client and `pumpkin-java-multiversion` translates. So the senders need no
    /// version check of their own to decide whether a value reaches a client.
    #[test]
    fn a_set_value_is_serialized_for_every_version() {
        let data = SynchedEntityData::new();
        data.define(PLAYER_MODE_CUSTOMISATION, 0u8);
        assert!(data.set(PLAYER_MODE_CUSTOMISATION, 0x7Fu8));

        let current = data.get_non_default_values_for_version(&JavaMinecraftVersion::V_26_3);
        let legacy = data.get_non_default_values_for_version(&JavaMinecraftVersion::V_1_20_5);
        assert!(current.is_some());
        assert_eq!(current, legacy);
        assert_eq!(
            data.pack_dirty_for_version(&JavaMinecraftVersion::V_1_20_5),
            current
        );
    }

    /// A value still at the default the client assumes is not sent.
    #[test]
    fn default_values_are_not_serialized() {
        let data = SynchedEntityData::new();
        data.define(PLAYER_MODE_CUSTOMISATION, 0u8);

        assert!(
            data.get_non_default_values_for_version(&JavaMinecraftVersion::V_26_3)
                .is_none()
        );
    }

    /// Like vanilla `isSetToDefault`, a value set back to its default is sent as a change but
    /// not to a client that starts tracking later, and a default defined after the value was
    /// set still counts.
    #[test]
    fn values_at_their_default_are_left_out_of_the_spawn_data() {
        let version = JavaMinecraftVersion::V_26_3;
        let data = SynchedEntityData::new();
        data.define(PLAYER_MODE_CUSTOMISATION, 0u8);
        assert!(data.set(PLAYER_MODE_CUSTOMISATION, 0x7Fu8));
        data.clear_dirty();
        assert!(data.set(PLAYER_MODE_CUSTOMISATION, 0u8));
        assert!(data.pack_dirty_for_version(&version).is_some());
        assert!(data.get_non_default_values_for_version(&version).is_none());

        let late = SynchedEntityData::new();
        late.set(PLAYER_MODE_CUSTOMISATION, 0u8);
        late.define(PLAYER_MODE_CUSTOMISATION, 0u8);
        assert!(late.get_non_default_values_for_version(&version).is_none());
        // Vanilla defines first, so setting the default again is no change to send.
        assert!(late.pack_dirty_for_version(&version).is_none());

        // A subclass default, like an overridden `getMaxAirSupply`, replaces the parent's
        // while nothing has set the value.
        let redefined = SynchedEntityData::new();
        redefined.define(PLAYER_MODE_CUSTOMISATION, 0u8);
        redefined.define(PLAYER_MODE_CUSTOMISATION, 0x7Fu8);
        assert!(
            redefined
                .get_non_default_values_for_version(&version)
                .is_none()
        );
    }
}
