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
