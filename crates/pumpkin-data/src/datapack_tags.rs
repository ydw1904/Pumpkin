//! The tags the loaded datapacks define, on top of the built-in ones in [`crate::tag`]
//! (vanilla `Registry.prepareTagReload` and `PendingTags.apply`).

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, PoisonError};

use arc_swap::ArcSwapOption;
use rustc_hash::FxHashMap;

use crate::tag::{RegistryKey, Tag, get_latest_map};

/// A loaded tag: its id, its values (without the `minecraft:` namespace, like the built-in
/// tags) and their registry ids.
pub type LoadedTag = (String, Vec<String>, Vec<u16>);

struct BoundTags {
    /// Every tag of each registry a datapack touched. A tag that failed to load is absent.
    registries: FxHashMap<RegistryKey, FxHashMap<&'static str, &'static Tag>>,
    /// The built-in tags whose contents changed, by the address of their `static`.
    builtin: FxHashMap<usize, &'static Tag>,
}

static BOUND: ArcSwapOption<BoundTags> = ArcSwapOption::const_empty();
/// Lets the built-in tags skip the lookup while no datapack touches any tag.
static ANY_BOUND: AtomicBool = AtomicBool::new(false);
static EMPTY: Tag = (&[], &[]);

/// Loaded tag contents, kept for the rest of the run so reloading the same packs again reuses
/// them instead of leaking a new copy.
#[derive(Default)]
struct Interner {
    strings: HashSet<&'static str>,
    tags: HashMap<(Vec<&'static str>, Vec<u16>), &'static Tag>,
}

static INTERNER: LazyLock<Mutex<Interner>> = LazyLock::new(Mutex::default);

impl Interner {
    fn string(&mut self, string: String) -> &'static str {
        if let Some(interned) = self.strings.get(string.as_str()) {
            return interned;
        }
        let interned: &'static str = Box::leak(string.into_boxed_str());
        self.strings.insert(interned);
        interned
    }

    fn tag(&mut self, values: Vec<String>, ids: Vec<u16>) -> &'static Tag {
        let values: Vec<&'static str> = values.into_iter().map(|v| self.string(v)).collect();
        let key = (values, ids);
        if let Some(interned) = self.tags.get(&key) {
            return interned;
        }
        let tag: &'static Tag = Box::leak(Box::new((
            Box::leak(key.0.clone().into_boxed_slice()) as &'static [&'static str],
            Box::leak(key.1.clone().into_boxed_slice()) as &'static [u16],
        )));
        self.tags.insert(key, tag);
        tag
    }
}

fn address(tag: &'static Tag) -> usize {
    std::ptr::from_ref(tag).addr()
}

/// Replaces the tags of the given registries with the loaded ones. Registries left out go back
/// to their built-in tags.
pub fn bind(registries: impl IntoIterator<Item = (RegistryKey, Vec<LoadedTag>)>) {
    let mut interner = INTERNER.lock().unwrap_or_else(PoisonError::into_inner);
    let mut bound = BoundTags {
        registries: FxHashMap::default(),
        builtin: FxHashMap::default(),
    };
    for (key, tags) in registries {
        let builtin_map = get_latest_map(key);
        let mut map = FxHashMap::default();
        for (name, values, ids) in tags {
            let builtin = builtin_map.get(name.as_str()).copied();
            let tag = match builtin {
                Some(builtin) if builtin.1 == ids.as_slice() && builtin.0 == values => builtin,
                _ => interner.tag(values, ids),
            };
            let name = builtin_map
                .get_key(name.as_str())
                .copied()
                .unwrap_or_else(|| interner.string(name));
            map.insert(name, tag);
        }
        for (name, builtin) in builtin_map.entries() {
            let tag = map.get(name).copied().unwrap_or(&EMPTY);
            if !std::ptr::eq(tag, *builtin) {
                bound.builtin.insert(address(builtin), tag);
            }
        }
        bound.registries.insert(key, map);
    }
    let any = !bound.registries.is_empty();
    BOUND.store(any.then(|| Arc::new(bound)));
    ANY_BOUND.store(any, Ordering::Release);
}

/// The contents of a built-in tag as the loaded datapacks define it.
#[must_use]
pub fn resolve(tag: &'static Tag) -> &'static Tag {
    if !ANY_BOUND.load(Ordering::Acquire) {
        return tag;
    }
    BOUND
        .load()
        .as_ref()
        .and_then(|bound| bound.builtin.get(&address(tag)).copied())
        .unwrap_or(tag)
}

/// A tag by its full id, as the loaded datapacks define it.
#[must_use]
pub fn get(key: RegistryKey, tag: &str) -> Option<&'static Tag> {
    if ANY_BOUND.load(Ordering::Acquire)
        && let Some(bound) = BOUND.load().as_ref()
        && let Some(tags) = bound.registries.get(&key)
    {
        return tags.get(tag).copied();
    }
    get_latest_map(key).get(tag).copied()
}

/// Every tag of a registry by its full id, as the loaded datapacks define them.
#[must_use]
pub fn tags(key: RegistryKey) -> Vec<(&'static str, &'static Tag)> {
    if ANY_BOUND.load(Ordering::Acquire)
        && let Some(bound) = BOUND.load().as_ref()
        && let Some(tags) = bound.registries.get(&key)
    {
        let mut tags: Vec<_> = tags.iter().map(|(name, tag)| (*name, *tag)).collect();
        tags.sort_unstable_by_key(|(name, _)| *name);
        return tags;
    }
    get_latest_map(key)
        .entries()
        .map(|(name, tag)| (*name, *tag))
        .collect()
}
