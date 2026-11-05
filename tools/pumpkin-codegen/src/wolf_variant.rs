use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use heck::ToPascalCase;
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use serde::Deserialize;

#[derive(Deserialize)]
struct WolfVariantJson {
    assets: WolfAssetsJson,
    #[serde(default)]
    baby_assets: Option<WolfAssetsJson>,
    #[serde(default)]
    spawn_conditions: Vec<SpawnConditionJson>,
}

#[derive(Deserialize)]
struct SpawnConditionJson {
    #[serde(default)]
    priority: i32,
    #[serde(default)]
    condition: Option<BiomeConditionJson>,
}

/// A `minecraft:biome` spawn condition: one biome, a `#` biome tag or a list of biomes.
#[derive(Deserialize)]
struct BiomeConditionJson {
    biomes: BiomesJson,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum BiomesJson {
    One(String),
    List(Vec<String>),
}

fn bare(name: &str) -> &str {
    name.strip_prefix("minecraft:").unwrap_or(name)
}

/// The biome names a condition matches, as a `&[&str]` expression.
fn biome_list_tokens(biomes: &BiomesJson) -> TokenStream {
    match biomes {
        BiomesJson::One(tag) if tag.starts_with('#') => {
            let tag_ident =
                format_ident!("{}", tag[1..].replace([':', '/'], "_").to_ascii_uppercase());
            quote! { crate::tag::WorldgenBiome::#tag_ident.0 }
        }
        BiomesJson::One(biome) => {
            let biome = bare(biome);
            quote! { &[#biome] }
        }
        BiomesJson::List(biomes) => {
            let biomes = biomes.iter().map(|biome| bare(biome));
            quote! { &[#(#biomes),*] }
        }
    }
}

#[derive(Deserialize)]
struct WolfAssetsJson {
    angry: String,
    tame: String,
    wild: String,
}

pub fn build() -> TokenStream {
    let dir = Path::new("../../assets/datapack/data/minecraft/wolf_variant");

    let mut variants: BTreeMap<String, WolfVariantJson> = BTreeMap::new();

    let mut entries = fs::read_dir(dir)
        .expect("read wolf_variant dir")
        .filter_map(Result::ok)
        .collect::<Vec<_>>();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let path = entry.path();
        if !path.extension().is_some_and(|ext| ext == "json") {
            continue;
        }

        let stem = path.file_stem().unwrap().to_string_lossy().to_string();
        let content = fs::read_to_string(&path).expect("read wolf_variant file");
        let json: WolfVariantJson =
            serde_json::from_str(&content).expect("parse wolf_variant JSON");

        variants.insert(stem, json);
    }

    let mut enum_variants = Vec::new();
    let mut from_name_arms = Vec::new();
    let mut to_name_arms = Vec::new();
    let mut asset_id_arms = Vec::new();
    let mut wild_asset_arms = Vec::new();
    let mut tame_asset_arms = Vec::new();
    let mut angry_asset_arms = Vec::new();
    let mut all_variants = Vec::new();
    let mut spawn_selectors = Vec::new();

    for (name, json) in &variants {
        let variant_ident = format_ident!("{}", name.to_pascal_case());
        let namespaced = format!("minecraft:{name}");
        let wild = &json.assets.wild;
        let tame = &json.assets.tame;
        let angry = &json.assets.angry;

        enum_variants.push(quote! { #variant_ident });
        from_name_arms.push(quote! {
            #namespaced | #name => Some(Self::#variant_ident)
        });
        to_name_arms.push(quote! {
            Self::#variant_ident => #name
        });
        asset_id_arms.push(quote! {
            Self::#variant_ident => #namespaced
        });
        wild_asset_arms.push(quote! {
            Self::#variant_ident => #wild
        });
        tame_asset_arms.push(quote! {
            Self::#variant_ident => #tame
        });
        angry_asset_arms.push(quote! {
            Self::#variant_ident => #angry
        });
        all_variants.push(quote! { Self::#variant_ident });
        for selector in &json.spawn_conditions {
            let priority = selector.priority;
            let biomes = selector.condition.as_ref().map_or_else(
                || quote! { None },
                |condition| {
                    let list = biome_list_tokens(&condition.biomes);
                    quote! { Some(#list) }
                },
            );
            spawn_selectors.push(quote! { (WolfVariant::#variant_ident, #priority, #biomes) });
        }
    }

    quote! {
        /* This file is generated. Do not edit manually. */

        #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
        #[repr(u8)]
        pub enum WolfVariant {
            #(#enum_variants,)*
        }

        impl WolfVariant {
            #[doc = "Returns the wolf variant from a resource name (bare or namespaced)."]
            #[must_use]
            pub fn from_name(name: &str) -> Option<Self> {
                match name {
                    #(#from_name_arms,)*
                    _ => None,
                }
            }

            #[doc = "Returns the numeric ID of the wolf variant in the synced registry."]
            #[must_use]
            pub const fn id(&self) -> u8 {
                *self as u8
            }

            #[doc = "Returns the bare string name of the wolf variant."]
            #[must_use]
            pub const fn to_name(&self) -> &'static str {
                match self {
                    #(#to_name_arms,)*
                }
            }

            #[doc = "Returns the fully-qualified asset id of the wolf variant."]
            #[must_use]
            pub const fn asset_id(&self) -> &'static str {
                match self {
                    #(#asset_id_arms,)*
                }
            }

            #[doc = "Returns the wild texture asset path."]
            #[must_use]
            pub const fn wild_texture(&self) -> &'static str {
                match self {
                    #(#wild_asset_arms,)*
                }
            }

            #[doc = "Returns the tame texture asset path."]
            #[must_use]
            pub const fn tame_texture(&self) -> &'static str {
                match self {
                    #(#tame_asset_arms,)*
                }
            }

            #[doc = "Returns the angry texture asset path."]
            #[must_use]
            pub const fn angry_texture(&self) -> &'static str {
                match self {
                    #(#angry_asset_arms,)*
                }
            }

            #[doc = "Returns all vanilla wolf variants."]
            #[must_use]
            pub const fn all() -> &'static [Self] {
                &[
                    #(#all_variants,)*
                ]
            }

            #[doc = "Vanilla `VariantUtils.selectVariantToSpawn` over the `spawn_conditions`: one of the matching selectors with the highest priority, chosen by `random_index` from their count."]
            #[must_use]
            pub fn select_to_spawn(
                biome_name: &str,
                random_index: impl FnOnce(usize) -> usize,
            ) -> Option<Self> {
                const SELECTORS: &[(WolfVariant, i32, Option<&[&str]>)] = &[
                    #(#spawn_selectors,)*
                ];
                let bare = biome_name.strip_prefix("minecraft:").unwrap_or(biome_name);
                let mut highest = i32::MIN;
                let mut matched = Vec::new();
                for &(variant, priority, biomes) in SELECTORS {
                    if priority < highest || biomes.is_some_and(|biomes| !biomes.contains(&bare)) {
                        continue;
                    }
                    if priority > highest {
                        highest = priority;
                        matched.clear();
                    }
                    matched.push(variant);
                }
                if matched.is_empty() {
                    return None;
                }
                matched.get(random_index(matched.len())).copied()
            }
        }
    }
}
