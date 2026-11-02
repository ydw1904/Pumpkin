use pumpkin_data::biome::Biome;
use pumpkin_data::item::Item;
pub use pumpkin_data::villager::{VillagerProfession, VillagerType};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::var_int::VarInt;
use serde::Serialize;

pub const BREEDING_FOOD_THRESHOLD: i32 = 12;

#[must_use]
pub fn get_food_points(item: &Item) -> i32 {
    use pumpkin_data::data_component::DataComponent;
    use pumpkin_data::data_component_impl::{VillagerFoodImpl, get};

    for (id, component) in item.components {
        if *id == DataComponent::VillagerFood {
            return get::<VillagerFoodImpl>(*component).nutrition;
        }
    }
    0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[repr(i32)]
pub enum GossipType {
    MajorNegative = 0,
    MinorNegative = 1,
    MinorPositive = 2,
    MajorPositive = 3,
    Trading = 4,
}

impl GossipType {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::MajorNegative => "major_negative",
            Self::MinorNegative => "minor_negative",
            Self::MinorPositive => "minor_positive",
            Self::MajorPositive => "major_positive",
            Self::Trading => "trading",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "major_negative" => Some(Self::MajorNegative),
            "minor_negative" => Some(Self::MinorNegative),
            "major_positive" => Some(Self::MajorPositive),
            "minor_positive" => Some(Self::MinorPositive),
            "trading" => Some(Self::Trading),
            _ => None,
        }
    }

    #[must_use]
    pub const fn from_legacy_id(id: i32) -> Option<Self> {
        match id {
            0 => Some(Self::MajorNegative),
            1 => Some(Self::MinorNegative),
            2 => Some(Self::MinorPositive),
            3 => Some(Self::MajorPositive),
            4 => Some(Self::Trading),
            _ => None,
        }
    }

    #[must_use]
    pub const fn weight(self) -> i32 {
        match self {
            Self::MajorNegative => -5,
            Self::MinorNegative => -1,
            Self::MajorPositive => 5,
            Self::MinorPositive | Self::Trading => 1,
        }
    }

    #[must_use]
    pub const fn max_value(self) -> i32 {
        match self {
            Self::MajorNegative => 100,
            Self::MinorNegative => 200,
            Self::MajorPositive => 20,
            Self::MinorPositive | Self::Trading => 25,
        }
    }

    /// Vanilla `GossipType.decayPerTransfer`: how much a gossip weakens when passed on.
    #[must_use]
    pub const fn decay_per_transfer(self) -> i32 {
        match self {
            Self::MajorNegative => 10,
            Self::MinorNegative | Self::MajorPositive | Self::Trading => 20,
            Self::MinorPositive => 5,
        }
    }

    #[must_use]
    pub const fn daily_decay(self) -> i32 {
        match self {
            Self::MajorNegative => 10,
            Self::MinorNegative => 20,
            Self::MajorPositive => 0,
            Self::MinorPositive => 1,
            Self::Trading => 2,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{GossipType, NbtCompound, VillagerData, VillagerProfession, VillagerType};

    #[test]
    fn villager_data_reads_vanilla_and_legacy_nbt() {
        let expected = VillagerData::new(VillagerType::Taiga, VillagerProfession::Librarian, 3);

        let mut vanilla = NbtCompound::new();
        vanilla.put_string("type", "minecraft:taiga".to_string());
        vanilla.put_string("profession", "minecraft:librarian".to_string());
        vanilla.put_int("level", 3);
        assert_eq!(VillagerData::from_nbt(&vanilla), expected);

        let mut legacy = NbtCompound::new();
        legacy.put_int("Type", VillagerType::Taiga as i32);
        legacy.put_int("Profession", VillagerProfession::Librarian as i32);
        legacy.put_int("Level", 3);
        assert_eq!(VillagerData::from_nbt(&legacy), expected);
    }

    #[test]
    fn gossip_types_use_vanilla_names_and_values() {
        let types = [
            (GossipType::MajorNegative, "major_negative", -5, 100, 10),
            (GossipType::MinorNegative, "minor_negative", -1, 200, 20),
            (GossipType::MinorPositive, "minor_positive", 1, 25, 1),
            (GossipType::MajorPositive, "major_positive", 5, 20, 0),
            (GossipType::Trading, "trading", 1, 25, 2),
        ];

        for (index, (gossip_type, name, weight, max, decay)) in types.into_iter().enumerate() {
            assert_eq!(gossip_type.name(), name);
            assert_eq!(GossipType::from_name(name), Some(gossip_type));
            assert_eq!(GossipType::from_legacy_id(index as i32), Some(gossip_type));
            assert_eq!(gossip_type.weight(), weight);
            assert_eq!(gossip_type.max_value(), max);
            assert_eq!(gossip_type.daily_decay(), decay);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VillagerData {
    pub r#type: VarInt,
    pub profession: VarInt,
    pub level: VarInt,
}

impl pumpkin_protocol::java::client::play::MetadataSerializer for VillagerData {
    fn write_metadata(
        &self,
        writer: &mut impl std::io::Write,
        _version: &pumpkin_util::version::JavaMinecraftVersion,
    ) -> Result<(), pumpkin_protocol::ser::WritingError> {
        use pumpkin_protocol::ser::NetworkWriteExt;
        writer.write_var_int(&self.r#type)?;
        writer.write_var_int(&self.profession)?;
        writer.write_var_int(&self.level)
    }
}

impl VillagerData {
    #[must_use]
    pub const fn new(r#type: VillagerType, profession: VillagerProfession, level: i32) -> Self {
        Self {
            r#type: VarInt(r#type as i32),
            profession: VarInt(profession as i32),
            level: VarInt(level),
        }
    }

    #[must_use]
    pub fn type_enum(&self) -> VillagerType {
        VillagerType::from_i32(self.r#type.0).unwrap_or(VillagerType::Plains)
    }

    #[must_use]
    pub fn profession_enum(&self) -> VillagerProfession {
        VillagerProfession::from_i32(self.profession.0).unwrap_or(VillagerProfession::None)
    }

    /// Vanilla `VillagerData.CODEC`: `type` and `profession` as ids, and `level`.
    #[must_use]
    pub fn to_nbt(&self) -> NbtCompound {
        let mut nbt = NbtCompound::new();
        nbt.put_string("type", format!("minecraft:{}", self.type_enum().to_name()));
        nbt.put_string(
            "profession",
            format!("minecraft:{}", self.profession_enum().to_name()),
        );
        nbt.put_int("level", self.level.0);
        nbt
    }

    /// Reads vanilla's format; worlds saved by older Pumpkin versions used `Type`, `Profession`
    /// and `Level` with numeric ids.
    #[must_use]
    pub fn from_nbt(nbt: &NbtCompound) -> Self {
        let r#type = nbt
            .get_string("type")
            .and_then(VillagerType::from_name)
            .or_else(|| nbt.get_int("Type").and_then(VillagerType::from_i32))
            .unwrap_or(VillagerType::Plains);
        let profession = nbt
            .get_string("profession")
            .and_then(VillagerProfession::from_name)
            .or_else(|| {
                nbt.get_int("Profession")
                    .and_then(VillagerProfession::from_i32)
            })
            .unwrap_or(VillagerProfession::None);
        let level = nbt
            .get_int("level")
            .or_else(|| nbt.get_int("Level"))
            .unwrap_or(1);
        Self::new(r#type, profession, level)
    }

    #[must_use]
    pub const fn with_type(mut self, r#type: VillagerType) -> Self {
        self.r#type = VarInt(r#type as i32);
        self
    }
}

/// Vanilla `VillagerType.byBiome`: biomes missing from its map give plains villagers.
#[must_use]
pub fn villager_type_by_biome(biome: &Biome) -> VillagerType {
    match biome.registry_id {
        "badlands" | "desert" | "eroded_badlands" | "wooded_badlands" => VillagerType::Desert,
        "bamboo_jungle" | "jungle" | "sparse_jungle" => VillagerType::Jungle,
        "savanna_plateau" | "savanna" | "windswept_savanna" => VillagerType::Savanna,
        "deep_frozen_ocean" | "frozen_ocean" | "frozen_river" | "ice_spikes" | "snowy_beach"
        | "snowy_taiga" | "snowy_plains" | "grove" | "snowy_slopes" | "frozen_peaks"
        | "jagged_peaks" => VillagerType::Snow,
        "swamp" | "mangrove_swamp" => VillagerType::Swamp,
        "old_growth_spruce_taiga"
        | "old_growth_pine_taiga"
        | "windswept_gravelly_hills"
        | "windswept_hills"
        | "taiga"
        | "windswept_forest" => VillagerType::Taiga,
        _ => VillagerType::Plains,
    }
}
