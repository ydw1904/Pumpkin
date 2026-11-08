use std::sync::Arc;

use pumpkin_data::entity::EntityType;

use crate::entity::EntityBase;
use crate::entity::mob::Mob;

/// Vanilla `ConversionParams` for `ConversionType.SINGLE`.
#[derive(Clone, Copy)]
pub struct ConversionParams {
    pub keep_equipment: bool,
    pub preserve_can_pick_up_loot: bool,
}

impl ConversionParams {
    #[must_use]
    pub const fn single(keep_equipment: bool, preserve_can_pick_up_loot: bool) -> Self {
        Self {
            keep_equipment,
            preserve_can_pick_up_loot,
        }
    }
}

/// Vanilla `Mob.convertTo`; see [`crate::entity::mob::MobEntity::convert_to_with`].
pub fn convert_to(
    from: &dyn Mob,
    entity_type: &'static EntityType,
    params: ConversionParams,
    after_conversion: impl FnOnce(&dyn Mob),
) -> Option<Arc<dyn EntityBase>> {
    let mut converted = None;
    from.get_mob_entity().convert_to_with(
        entity_type,
        params.keep_equipment,
        params.preserve_can_pick_up_loot,
        |to| {
            if let Some(mob) = to.get_mob() {
                after_conversion(mob);
            }
            converted = Some(to.clone());
        },
    );
    converted
}

/// Vanilla `ConversionType.SPLIT_ON_DEATH`: spawns one split-off child of a dying mob. Removal
/// already detached its riders and leash, so only the common data is copied.
pub fn split_on_death(
    from: &dyn Mob,
    entity_type: &'static EntityType,
    after_conversion: impl FnOnce(&dyn Mob),
) {
    let from_entity = from.get_entity();
    let world = from_entity.world.load_full();
    let converted = crate::entity::r#type::from_type(
        entity_type,
        from_entity.pos.load(),
        &world,
        uuid::Uuid::new_v4(),
    );
    let Some(to) = converted.get_mob() else {
        return;
    };
    from.get_mob_entity().convert_common(to, false, false);
    after_conversion(to);
    world.spawn_entity(converted);
}
