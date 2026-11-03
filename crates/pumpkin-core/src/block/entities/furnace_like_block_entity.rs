use pumpkin_data::{item_stack::ItemStack, recipes::CookingRecipe};

use crate::block::entities::{BlockEntity, PropertyDelegate};
pub use pumpkin_inventory::ExperienceContainer;
use pumpkin_inventory::{Clearable, Inventory};

/// Trait for extracting smelting experience from cooking block entities.
pub trait CookingBlockEntityBase:
    Sync + Send + Inventory + PropertyDelegate + BlockEntity + Clearable
{
    fn get_cooking_time_spent(&self) -> u16;
    fn get_cooking_total_time(&self) -> u16;
    fn get_lit_time_remaining(&self) -> u16;
    fn get_lit_total_time(&self) -> u16;

    /// Track that a recipe was used (for XP calculation on extraction)
    /// Uses the result item ID as the recipe identifier
    fn add_recipe_used(&self, recipe: &CookingRecipe);
    /// Extract and reset accumulated experience, returning the total as an integer
    /// Calculates XP from tracked recipes and clears the `recipes_used` map
    fn extract_experience_from_recipes(&self) -> i32;

    fn get_input_item(&self) -> ItemStack;
    fn get_fuel_item(&self) -> ItemStack;
    fn get_output_item(&self) -> ItemStack;

    fn set_cooking_time_spent(&self, spent_time: u16);
    fn set_cooking_total_time(&self, total_time: u16);
    fn set_lit_time_remaining(&self, remaining_time: u16);
    fn set_lit_total_time(&self, total_time: u16);

    fn is_burning(&self) -> bool;
    /// The recipe's cooking time divided by the current fuel's speed multiplier.
    ///
    /// Mojang name: `getTotalCookTime`
    fn total_cook_time(&self, recipe: &CookingRecipe) -> u16;
    fn can_accept_recipe_output(&self, recipe: Option<&CookingRecipe>, max_count: u8) -> bool;
    fn craft_recipe(&self, recipe: Option<&CookingRecipe>) -> bool;
}

#[macro_export]
macro_rules! impl_cooking_block_entity_base {
    ($struct_name:ty) => {
        impl CookingBlockEntityBase for $struct_name {
            fn get_cooking_time_spent(&self) -> u16 {
                self.cooking_time_spent.load(Ordering::Relaxed)
            }

            fn get_cooking_total_time(&self) -> u16 {
                self.cooking_total_time.load(Ordering::Relaxed)
            }

            fn get_lit_time_remaining(&self) -> u16 {
                self.lit_time_remaining.load(Ordering::Relaxed)
            }

            fn get_lit_total_time(&self) -> u16 {
                self.lit_total_time.load(Ordering::Relaxed)
            }

            fn get_input_item(&self) -> ItemStack {
                let items = self
                    .items
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                items[0].clone()
            }

            fn get_fuel_item(&self) -> ItemStack {
                let items = self
                    .items
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                items[1].clone()
            }

            fn get_output_item(&self) -> ItemStack {
                let items = self
                    .items
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                items[2].clone()
            }

            fn set_cooking_time_spent(&self, spent_time: u16) {
                self.cooking_time_spent.store(spent_time, Ordering::Relaxed);
            }

            fn set_cooking_total_time(&self, total_time: u16) {
                self.cooking_total_time.store(total_time, Ordering::Relaxed);
            }

            fn set_lit_time_remaining(&self, remaining_time: u16) {
                self.lit_time_remaining
                    .store(remaining_time, Ordering::Relaxed);
            }

            fn set_lit_total_time(&self, total_time: u16) {
                self.lit_total_time.store(total_time, Ordering::Relaxed);
            }

            fn is_burning(&self) -> bool {
                self.get_lit_time_remaining() > 0
            }

            fn total_cook_time(&self, recipe: &pumpkin_data::recipes::CookingRecipe) -> u16 {
                let speed = f32::from_bits(self.speed_multiplier.load(Ordering::Relaxed));
                if speed > 0.0 {
                    (recipe.cookingtime as f32 / speed).ceil() as u16
                } else {
                    recipe.cookingtime as u16
                }
            }

            fn add_recipe_used(&self, recipe: &pumpkin_data::recipes::CookingRecipe) {
                // Track recipe usage by recipe ID for XP calculation
                let recipe_id = recipe.recipe_id.to_string();
                let mut recipes = self
                    .recipes_used
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                *recipes.entry(recipe_id).or_insert(0) += 1;
            }

            fn extract_experience_from_recipes(&self) -> i32 {
                // Calculate total XP from tracked recipes and clear the map (vanilla behavior)
                let mut recipes = self
                    .recipes_used
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let mut total_xp: f32 = 0.0;
                for (recipe_id, count) in recipes.iter() {
                    // Look up the recipe's XP value
                    if let Some(xp) = pumpkin_data::recipes::get_recipe_experience(recipe_id) {
                        total_xp += xp * (*count as f32);
                    }
                }
                recipes.clear();
                total_xp.floor() as i32
            }

            fn can_accept_recipe_output(
                &self,
                recipe: Option<&pumpkin_data::recipes::CookingRecipe>,
                max_count: u8,
            ) -> bool {
                let Some(recipe) = recipe else { return false };
                let Ok(items) = self.items.try_read() else {
                    return false;
                };

                let is_top_items_empty = items[0].is_empty();
                let side_item_stack = &items[2];

                if side_item_stack.is_empty() {
                    return !is_top_items_empty;
                }

                if let Some(recipe_output_item) = pumpkin_data::item::Item::from_registry_key(
                    recipe
                        .result
                        .id
                        .strip_prefix("minecraft:")
                        .unwrap_or(&recipe.result.id),
                ) && !is_top_items_empty
                    && recipe_output_item.id == side_item_stack.item.id
                    && side_item_stack.item_count < max_count
                    && side_item_stack.item_count < side_item_stack.get_max_stack_size()
                {
                    return true;
                }
                false
            }
            fn craft_recipe(&self, recipe: Option<&pumpkin_data::recipes::CookingRecipe>) -> bool {
                let can_accept_output =
                    self.can_accept_recipe_output(recipe, self.get_max_count_per_stack());
                if let Some(recipe) = recipe {
                    if can_accept_output {
                        let Ok(mut items) = self.items.try_write() else {
                            return false;
                        };
                        let Some(output_item) = pumpkin_data::item::Item::from_registry_key(
                            recipe
                                .result
                                .id
                                .strip_prefix("minecraft:")
                                .unwrap_or(&recipe.result.id),
                        ) else {
                            return false;
                        };
                        let output_item_stack = ItemStack::new(recipe.result.count, output_item);

                        if items[2].are_equal(ItemStack::EMPTY) {
                            items[2] = output_item_stack;
                        } else if items[2].are_items_and_components_equal(&output_item_stack) {
                            items[2].increment(1);
                        }

                        // Track recipe usage for XP calculation (vanilla RecipesUsed format)
                        self.add_recipe_used(recipe);

                        if items[0].item.id == pumpkin_data::item::Item::WET_SPONGE.id
                            && !items[1].is_empty()
                            && items[1].item.id == pumpkin_data::item::Item::BUCKET.id
                        {
                            items[1] = ItemStack::new(1, &pumpkin_data::item::Item::WATER_BUCKET);
                        }

                        items[0].decrement(1);
                        return true;
                    }
                }

                false
            }
        }
    };
}

#[macro_export]
macro_rules! impl_property_delegate_for_cooking {
    ($struct_name:ty) => {
        impl $crate::block::entities::PropertyDelegate for $struct_name {
            fn get_property(&self, index: i32) -> i32 {
                match index {
                    0 => self.get_lit_time_remaining() as i32,
                    1 => self.get_lit_total_time() as i32,
                    2 => self.get_cooking_time_spent() as i32,
                    3 => self.get_cooking_total_time() as i32,
                    _ => 0,
                }
            }

            fn set_property(&self, index: i32, value: i32) {
                let value = value as u16;
                match index {
                    0 => self.set_lit_time_remaining(value),
                    1 => self.set_lit_total_time(value),
                    2 => self.set_cooking_time_spent(value),
                    3 => self.set_cooking_total_time(value),
                    _ => {}
                }
            }

            fn get_properties_size(&self) -> i32 {
                4
            }
        }
    };
}

#[macro_export]
macro_rules! impl_clearable_for_cooking {
    ($struct_name:ty) => {
        impl pumpkin_inventory::Clearable for $struct_name {
            fn clear(&self) {
                let mut items = self
                    .items
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                items.fill_with(|| ItemStack::EMPTY.clone());
                self.mark_dirty();
            }
        }
    };
}

#[macro_export]
macro_rules! impl_experience_container_for_cooking {
    ($struct_name:ty) => {
        impl $crate::block::entities::furnace_like_block_entity::ExperienceContainer
            for $struct_name
        {
            fn extract_experience(&self) -> i32 {
                // Delegate to the CookingBlockEntityBase method
                self.extract_experience_from_recipes()
            }

            fn used_recipe_ids(&self) -> Vec<String> {
                self.recipes_used
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .keys()
                    .cloned()
                    .collect()
            }
        }
    };
}

#[macro_export]
macro_rules! impl_inventory_for_cooking {
    ($struct_name:ty, $recipe_kind:expr) => {
        impl pumpkin_inventory::Inventory for $struct_name {
            fn size(&self) -> usize {
                Self::INVENTORY_SIZE
            }

            fn is_empty(&self) -> bool {
                let items = self
                    .items
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                items.iter().all(|s| s.is_empty())
            }

            fn get_stack(&self, slot: usize) -> ItemStack {
                let items = self
                    .items
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                items[slot].clone()
            }

            fn remove_stack(&self, slot: usize) -> ItemStack {
                let mut items = self
                    .items
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let removed = std::mem::replace(&mut items[slot], ItemStack::EMPTY.clone());
                self.mark_dirty();
                removed
            }

            fn remove_stack_specific(&self, slot: usize, amount: u8) -> ItemStack {
                let mut items = self
                    .items
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let res = if !items[slot].is_empty() && amount > 0 {
                    items[slot].split(amount)
                } else {
                    ItemStack::EMPTY.clone()
                };
                self.mark_dirty();
                res
            }

            fn set_stack(&self, slot: usize, stack: ItemStack) {
                let mut items = self
                    .items
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let is_same_item = !stack.is_empty()
                    && ItemStack::are_items_and_components_equal(&items[slot], &stack);

                items[slot] = stack.clone();

                if slot == 0 && !is_same_item {
                    // AbstractFurnaceBlockEntity.getTotalCookTime: 200 when nothing cooks.
                    let total = pumpkin_data::recipes::get_cooking_recipe_with_ingredient(
                        stack.item,
                        $recipe_kind,
                    )
                    .map_or(200, |recipe| self.total_cook_time(recipe));
                    self.set_cooking_total_time(total);
                    self.set_cooking_time_spent(0);
                }

                // Always consider the inventory changed when setting a stack
                self.mark_dirty();
            }

            fn mark_dirty(&self) {
                self.dirty.store(true, Ordering::Relaxed);
                self.comparator_dirty.store(true, Ordering::Relaxed);
            }

            /// AbstractFurnaceBlockEntity.canPlaceItem: nothing goes into the result, the fuel
            /// slot takes fuel or one empty bucket.
            fn is_valid_slot_for(&self, slot: usize, stack: &ItemStack) -> bool {
                match slot {
                    2 => false,
                    1 => stack
                        .get_data_component::<pumpkin_data::data_component_impl::CookingFuelImpl>()
                        .is_some()
                        || (stack.item == &pumpkin_data::item::Item::BUCKET
                            && self.get_fuel_item().item != &pumpkin_data::item::Item::BUCKET),
                    _ => true,
                }
            }

            fn slots_for_face(
                &self,
                side: pumpkin_data::BlockDirection,
            ) -> Option<&'static [usize]> {
                Some(match side {
                    pumpkin_data::BlockDirection::Down => &[2, 1],
                    pumpkin_data::BlockDirection::Up => &[0],
                    _ => &[1],
                })
            }

            /// Only empty and water buckets come back out of the fuel slot from below.
            fn can_take_item_through_face(
                &self,
                slot: usize,
                stack: &ItemStack,
                side: pumpkin_data::BlockDirection,
            ) -> bool {
                use pumpkin_data::tag::Taggable;
                side != pumpkin_data::BlockDirection::Down
                    || slot != 1
                    || stack
                        .item
                        .has_tag(&pumpkin_data::tag::Item::MINECRAFT_FURNACE_FUEL_BOTTOM_TAKEABLE)
            }

            fn as_any(&self) -> &dyn std::any::Any {
                self
            }
        }
    };
}

#[macro_export]
macro_rules! impl_block_entity_for_cooking {
    ($struct_name:ty,$recipe_kind:expr) => {
        impl $crate::block::entities::BlockEntity for $struct_name {
            #[expect(clippy::too_many_lines)]
            fn tick(
                &self,
                world: &Arc<$crate::world::World>,
            ) {
                let is_burning = self.is_burning();
                let mut is_dirty = false;
                if self.is_burning() {
                    self.lit_time_remaining.fetch_sub(1, Ordering::Relaxed);
                }

                let (top_item, bottom_item) = if let Ok(items_guard) = self.items.try_read() {
                    (items_guard[0].clone(), items_guard[1].clone())
                } else {
                    return;
                };

                let is_top_items_empty = top_item.is_empty();

                let furnace_recipe = pumpkin_data::recipes::get_cooking_recipe_with_ingredient(
                    top_item.item,
                    $recipe_kind,
                );

                let can_accept_output = self
                    .can_accept_recipe_output(furnace_recipe, self.get_max_count_per_stack());

                let bottom_items_is_empty = bottom_item.is_empty();
                if self.is_burning() || !bottom_items_is_empty && !is_top_items_empty {
                    if !self.is_burning() && can_accept_output {
                        let is_fast = matches!(
                            $recipe_kind,
                            CookingRecipeKind::Blasting | CookingRecipeKind::Smoking
                        );
                        let fuel_component = bottom_item
                            .get_data_component::<pumpkin_data::data_component_impl::CookingFuelImpl>();
                        let adjusted_fuel_ticks = fuel_component
                            .map_or(0, |fuel| fuel.get_burn_ticks(is_fast));

                        let mut burn_ticks = adjusted_fuel_ticks;
                        let mut burn_cancelled = false;
                        if let Some(server) = world.server.upgrade() {
                            let mut burn_event = $crate::plugin::api::events::inventory::furnace_burn::FurnaceBurnEvent::new(
                                self.position,
                                bottom_item.item.registry_key.to_string(),
                                adjusted_fuel_ticks as u32,
                            );
                            server.plugin_manager.fire_blocking(&server, &mut burn_event);
                            if burn_event.cancelled {
                                burn_cancelled = true;
                            } else {
                                burn_ticks = burn_event.burn_time as u16;
                            }
                        }
                        if !burn_cancelled {
                            self.set_lit_time_remaining(burn_ticks);
                            self.set_lit_total_time(burn_ticks);
                            // 26.3 fuels carry a speed multiplier (2 in blast furnaces and
                            // smokers) that divides the recipe's cooking time; progress made
                            // so far keeps its ratio.
                            let speed = fuel_component
                                .map_or(1.0, |fuel| fuel.get_speed_multiplier(is_fast));
                            self.speed_multiplier.store(speed.to_bits(), Ordering::Relaxed);
                            let total = self.get_cooking_total_time();
                            let spent = self.get_cooking_time_spent();
                            if total > 0 && spent < total
                                && let Some(recipe) = furnace_recipe
                            {
                                let ratio = f32::from(spent) / f32::from(total);
                                let new_total = self.total_cook_time(recipe);
                                self.set_cooking_total_time(new_total);
                                self.set_cooking_time_spent(
                                    (ratio * f32::from(new_total)).ceil() as u16,
                                );
                            }

                            if self.is_burning() {
                                is_dirty = true;
                                if let Ok(mut items_guard) = self.items.try_write() {
                                    if !items_guard[1].is_empty() {
                                        // consumeFuel: the item's crafting remainder replaces
                                        // the last fuel item, or drops if fuel is left.
                                        let remainder = pumpkin_data::item_stack::crafting_remainder(
                                            items_guard[1].item,
                                        );
                                        items_guard[1].decrement(1);
                                        if let Some(remainder_item) = remainder
                                        {
                                            let remainder_stack = ItemStack::new(1, remainder_item);
                                            if items_guard[1].is_empty() {
                                                items_guard[1] = remainder_stack;
                                            } else {
                                                let pos = self.position.to_centered_f64();
                                                let entity = $crate::entity::Entity::new(
                                                    world.clone(),
                                                    pos,
                                                    &pumpkin_data::entity::EntityType::ITEM,
                                                );
                                                world.spawn_entity(std::sync::Arc::new(
                                                    $crate::entity::item::ItemEntity::new(
                                                        entity,
                                                        remainder_stack,
                                                    ),
                                                ));
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    if self.is_burning() && can_accept_output {
                        let mut start_cancelled = false;
                        if self.get_cooking_time_spent() == 0 {
                            if let Some(server) = world.server.upgrade() {
                                let mut start_event = $crate::plugin::api::events::inventory::furnace_start_smelt::FurnaceStartSmeltEvent::new(
                                    self.position,
                                    top_item.item.registry_key.to_string(),
                                    self.get_cooking_total_time() as u32,
                                );
                                server.plugin_manager.fire_blocking(&server, &mut start_event);
                                if start_event.cancelled {
                                    start_cancelled = true;
                                } else {
                                    self.set_cooking_total_time(start_event.cooking_time as u16);
                                }
                            }
                        }
                        if !start_cancelled {
                            self.cooking_time_spent.fetch_add(1, Ordering::Relaxed);

                            if self.get_cooking_time_spent() >= self.get_cooking_total_time() {
                                self.set_cooking_time_spent(0);
                                if let Some(cooking_recipe) = furnace_recipe {
                                    self.set_cooking_total_time(self.total_cook_time(cooking_recipe));

                                    let mut smelt_cancelled = false;
                                    if let Some(server) = world.server.upgrade() {
                                        let mut smelt_event = $crate::plugin::api::events::inventory::furnace_smelt::FurnaceSmeltEvent::new(
                                            self.position,
                                            top_item.item.registry_key.to_string(),
                                            cooking_recipe.result.id.to_string(),
                                        );
                                        server.plugin_manager.fire_blocking(&server, &mut smelt_event);
                                        if smelt_event.cancelled {
                                            smelt_cancelled = true;
                                        }
                                    }
                                    if !smelt_cancelled {
                                        self.craft_recipe(Some(cooking_recipe));
                                        is_dirty = true;
                                    }
                                }
                            }
                        }
                    } else {
                        self.set_cooking_time_spent(0);
                    }
                } else if !self.is_burning() && self.get_cooking_time_spent() > 0 {
                    let _ = self.cooking_time_spent.try_update(
                        Ordering::Acquire,
                        Ordering::Acquire,
                        |v| {
                            Some(
                                v.saturating_sub(2)
                                    .min(self.cooking_total_time.load(Ordering::Acquire)),
                            )
                        },
                    );
                }

                if is_burning != self.is_burning() {
                    is_dirty = true;
                    let (furnace_block, furnace_block_state) =
                        world.get_block_and_state(&self.position);
                    let mut props =
                        pumpkin_data::block_properties::FurnaceLikeProperties::from_state_id(furnace_block_state.id);

                    props.lit = self.is_burning();
                    world.set_block_state(
                        &self.position,
                        props.to_state_id(furnace_block),
                        $crate::world::BlockFlags::NOTIFY_ALL,
                    );
                }

                if is_dirty {
                    self.mark_dirty();
                }
            }

            fn resource_location(&self) -> &'static str {
                Self::ID
            }

            fn get_position(&self) -> BlockPos {
                self.position
            }

            fn from_nbt(nbt: &pumpkin_nbt::compound::NbtCompound, position: BlockPos) -> Self
            where
                Self: Sized,
            {
                let cooking_total_time = AtomicU16::new(
                    nbt.get_numeric_int("cooking_total_time")
                        .map_or(0, |cooking_total_time| cooking_total_time as u16),
                );
                let cooking_time_spent = AtomicU16::new(
                    nbt.get_numeric_int("cooking_time_spent")
                        .map_or(0, |cooking_time_spent| cooking_time_spent as u16),
                );
                let lit_total_time = AtomicU16::new(
                    nbt.get_numeric_int("lit_total_time")
                        .map_or(0, |lit_total_time| lit_total_time as u16),
                );
                let lit_time_remaining = AtomicU16::new(
                    nbt.get_numeric_int("lit_time_remaining")
                        .map_or(0, |lit_time_remaining| lit_time_remaining as u16),
                );
                // Load RecipesUsed from NBT (vanilla format: map of recipe ID -> craft count)
                let mut recipes_used_map = HashMap::new();
                if let Some(recipes_compound) = nbt.get_compound("RecipesUsed") {
                    for (recipe_id, tag) in &recipes_compound.child_tags {
                        if let pumpkin_nbt::tag::NbtTag::Int(count) = tag {
                            recipes_used_map.insert(recipe_id.to_string(), *count as u32);
                        }
                    }
                }

                let speed_multiplier = std::sync::atomic::AtomicU32::new(
                    nbt.get_float("speed_multiplier").unwrap_or(1.0).to_bits(),
                );

                let mut furnace = Self {
                    speed_multiplier,
                    position,
                    dirty: AtomicBool::new(false),
                    comparator_dirty: AtomicBool::new(false),
                    items: std::sync::RwLock::new(from_fn(|_| ItemStack::EMPTY.clone())),
                    cooking_total_time,
                    cooking_time_spent,
                    lit_total_time,
                    lit_time_remaining,
                    recipes_used: std::sync::Mutex::new(recipes_used_map),
                };
                pumpkin_inventory::sync_read_items_from_nbt(nbt, furnace.items.get_mut().unwrap_or_else(std::sync::PoisonError::into_inner));

                furnace
            }

            fn write_nbt(&self, nbt: &mut pumpkin_nbt::compound::NbtCompound) {
                nbt.put_int("cooking_total_time", i32::from(self.get_cooking_total_time()));
                nbt.put_int("cooking_time_spent", i32::from(self.get_cooking_time_spent()));
                nbt.put_int("lit_total_time", i32::from(self.get_lit_total_time()));
                nbt.put_int("lit_time_remaining", i32::from(self.get_lit_time_remaining()));
                nbt.put_float(
                    "speed_multiplier",
                    f32::from_bits(self.speed_multiplier.load(Ordering::Relaxed)),
                );

                // Save RecipesUsed in vanilla format (map of recipe ID -> craft count)
                {
                    let recipes = self
                        .recipes_used
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if !recipes.is_empty() {
                        let mut recipes_compound = pumpkin_nbt::compound::NbtCompound::new();
                        for (recipe_id, count) in recipes.iter() {
                            recipes_compound.put(
                                recipe_id.as_str(),
                                pumpkin_nbt::tag::NbtTag::Int(*count as i32),
                            );
                        }
                        nbt.put(
                            "RecipesUsed",
                            pumpkin_nbt::tag::NbtTag::Compound(recipes_compound),
                        );
                    }
                }

                self.write_inventory_nbt(nbt, true);
            }

            fn get_inventory(
                self: Arc<Self>,
            ) -> Option<Arc<dyn pumpkin_inventory::Inventory>> {
                Some(self)
            }

            fn is_comparator_dirty(&self) -> bool {
                self.comparator_dirty.load(Ordering::Relaxed)
            }

            fn clear_comparator_dirty(&self) {
                self.comparator_dirty.store(false, Ordering::Relaxed);
            }


            fn is_dirty(&self) -> bool {
                self.dirty.load(Ordering::Relaxed)
            }

            fn bedrock_block_actor_data(&self, _state_id: pumpkin_data::BlockStateId) -> Option<pumpkin_nbt::compound::NbtCompound> {
                let bedrock_id = match $recipe_kind {
                    pumpkin_data::recipes::CookingRecipeKind::Smelting => "Furnace",
                    pumpkin_data::recipes::CookingRecipeKind::Blasting => "BlastFurnace",
                    pumpkin_data::recipes::CookingRecipeKind::Smoking => "Smoker",
                    _ => "Furnace",
                };
                Some($crate::block::entities::furnace_like_block_entity::build_furnace_actor(
                    bedrock_id,
                    self.position,
                    self.get_cooking_time_spent(),
                    self.get_lit_time_remaining(),
                    self.get_lit_total_time(),
                    0,
                    None,
                ))
            }

            fn as_any(&self) -> &dyn std::any::Any {
                self
            }

            fn to_property_delegate(
                self: Arc<Self>,
            ) -> Option<Arc<dyn $crate::block::entities::PropertyDelegate>> {
                Some(self as Arc<dyn $crate::block::entities::PropertyDelegate>)
            }

            fn to_experience_container(
                self: Arc<Self>,
            ) -> Option<
                Arc<dyn $crate::block::entities::furnace_like_block_entity::ExperienceContainer>,
            > {
                Some(
                    self as Arc<
                        dyn $crate::block::entities::furnace_like_block_entity::ExperienceContainer,
                    >,
                )
            }
        }
    };
}

/// Builds the Bedrock block-actor NBT for cooking appliances (Furnace, Smoker, `BlastFurnace`).
#[must_use]
pub fn build_furnace_actor(
    id: &'static str,
    position: pumpkin_util::math::position::BlockPos,
    cook_time: u16,
    lit_remaining: u16,
    lit_total: u16,
    stored_xp: i32,
    custom_name: Option<&str>,
) -> pumpkin_nbt::compound::NbtCompound {
    super::bedrock::BedrockBlockActorBuilder::new(id, position)
        .custom_name(custom_name)
        .put_short("BurnTime", lit_remaining as i16)
        .put_short("BurnDuration", lit_total as i16)
        .put_short("CookTime", cook_time as i16)
        .put_short("StoredXPInt", stored_xp.clamp(0, i16::MAX as i32) as i16)
        .finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pumpkin_util::math::position::BlockPos;

    #[test]
    fn furnace_actor_builder() {
        let pos = BlockPos::new(10, 60, 20);
        let nbt = build_furnace_actor("Smoker", pos, 150, 40, 200, 5, Some("Custom Smoker"));
        assert_eq!(nbt.get_string("id"), Some("Smoker"));
        assert_eq!(nbt.get_string("CustomName"), Some("Custom Smoker"));
        assert_eq!(nbt.get_short("CookTime"), Some(150));
        assert_eq!(nbt.get_short("BurnTime"), Some(40));
        assert_eq!(nbt.get_short("BurnDuration"), Some(200));
        assert_eq!(nbt.get_short("StoredXPInt"), Some(5));
        assert_eq!(nbt.get_bool("isMovable"), Some(true));
    }
}
