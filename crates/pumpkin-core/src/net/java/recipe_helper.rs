use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::recipes::RecipeIngredientTypes;
use pumpkin_inventory::player::player_inventory::PlayerInventory;
use pumpkin_protocol::codec::recipe::OwnedRecipeIngredient;

#[derive(Clone, Copy)]
pub enum GenericIngredient<'a> {
    Vanilla(&'a RecipeIngredientTypes),
    Dynamic(&'a OwnedRecipeIngredient),
}

impl GenericIngredient<'_> {
    #[must_use]
    pub fn match_item(&self, item: &Item) -> bool {
        match self {
            Self::Vanilla(v) => v.match_item(item),
            Self::Dynamic(d) => d.match_item(item),
        }
    }
}

/// Vanilla `PlacementInfo`: the ingredients a recipe takes and which ingredient each recipe slot
/// holds.
pub struct PlacementInfo<'a> {
    pub ingredients: Vec<GenericIngredient<'a>>,
    /// The ingredient index of every recipe slot, `None` for an empty one.
    pub slots_to_ingredient_index: Vec<Option<usize>>,
    /// Width and height of a shaped recipe; other recipes fill the grid in order.
    pub shaped_size: Option<(usize, usize)>,
}

impl<'a> PlacementInfo<'a> {
    /// `PlacementInfo.create`: every ingredient in its own slot.
    #[must_use]
    pub fn create(ingredients: Vec<GenericIngredient<'a>>) -> Self {
        let slots_to_ingredient_index = (0..ingredients.len()).map(Some).collect();
        Self {
            ingredients,
            slots_to_ingredient_index,
            shaped_size: None,
        }
    }

    /// `PlacementInfo.createFromOptionals` for a shaped recipe's slots, row by row.
    #[must_use]
    pub fn shaped(width: usize, height: usize, slots: Vec<Option<GenericIngredient<'a>>>) -> Self {
        let mut ingredients = Vec::new();
        let slots_to_ingredient_index = slots
            .into_iter()
            .map(|slot| {
                slot.map(|ingredient| {
                    ingredients.push(ingredient);
                    ingredients.len() - 1
                })
            })
            .collect();
        Self {
            ingredients,
            slots_to_ingredient_index,
            shaped_size: Some((width, height)),
        }
    }

    #[must_use]
    pub const fn is_impossible_to_place(&self) -> bool {
        self.slots_to_ingredient_index.is_empty()
    }
}

/// `Inventory.isUsableForCrafting`.
#[must_use]
pub fn is_usable_for_crafting(stack: &ItemStack) -> bool {
    (!stack.is_damageable() || stack.get_damage() <= 0)
        && !stack.has_enchantments()
        && !stack.has_custom_name()
}

/// Vanilla `StackedItemContents`: how many of each item are available for a recipe.
///
/// Items are kept in the order they were first counted. Vanilla iterates an identity hash map, so
/// which of several matching items gets picked can differ.
#[derive(Default)]
pub struct StackedItemContents {
    amounts: Vec<(&'static Item, u32)>,
}

impl StackedItemContents {
    /// `accountSimpleStack`: counts the stack if it is usable for crafting.
    pub fn account_simple_stack(&mut self, stack: &ItemStack) {
        if is_usable_for_crafting(stack) {
            self.account_stack(stack);
        }
    }

    /// `accountStack`: counts up to a stack's worth of the item.
    pub fn account_stack(&mut self, stack: &ItemStack) {
        if !stack.is_empty() {
            let count = stack.item_count.min(stack.get_max_stack_size());
            self.put(stack.item, u32::from(count));
        }
    }

    /// `canCraft`: whether `amount` crafts' worth can be taken, pushing the item used for each
    /// ingredient to `output` when they can.
    pub fn can_craft(
        &mut self,
        placement: &PlacementInfo<'_>,
        amount: u32,
        output: Option<&mut Vec<&'static Item>>,
    ) -> bool {
        !placement.is_impossible_to_place()
            && RecipePicker::new(self, &placement.ingredients).try_pick(amount, output)
    }

    /// `getBiggestCraftableStack`.
    pub fn biggest_craftable_stack(&mut self, placement: &PlacementInfo<'_>) -> u32 {
        RecipePicker::new(self, &placement.ingredients).try_pick_all(u32::MAX, None)
    }

    fn amount(&self, item: &Item) -> u32 {
        self.amounts
            .iter()
            .find(|(other, _)| other.id == item.id)
            .map_or(0, |(_, count)| *count)
    }

    fn put(&mut self, item: &'static Item, count: u32) {
        if let Some(entry) = self
            .amounts
            .iter_mut()
            .find(|(other, _)| other.id == item.id)
        {
            entry.1 += count;
        } else {
            self.amounts.push((item, count));
        }
    }

    fn take(&mut self, item: &Item, count: u32) {
        if let Some(entry) = self
            .amounts
            .iter_mut()
            .find(|(other, _)| other.id == item.id)
        {
            entry.1 = entry.1.saturating_sub(count);
        }
    }

    /// `getResultUpperBound`.
    fn result_upper_bound(&self, ingredients: &[GenericIngredient<'_>]) -> u32 {
        let mut min = u32::MAX;
        'ingredients: for ingredient in ingredients {
            let mut max = 0;
            for (item, count) in &self.amounts {
                if *count > max {
                    if ingredient.match_item(item) {
                        max = *count;
                    }
                    if max >= min {
                        continue 'ingredients;
                    }
                }
            }
            min = max;
            if max == 0 {
                break;
            }
        }
        min
    }
}

/// `StackedContents.RecipePicker`: assigns available items to ingredients by augmenting paths.
struct RecipePicker<'c, 'i> {
    contents: &'c mut StackedItemContents,
    ingredients: &'i [GenericIngredient<'i>],
    items: Vec<&'static Item>,
    visited_ingredients: Vec<bool>,
    visited_items: Vec<bool>,
    satisfied: Vec<bool>,
    /// Indexed `item * ingredient_count + ingredient`.
    connections: Vec<bool>,
    /// Indexed like `connections`.
    assigned: Vec<bool>,
    path: Vec<usize>,
}

impl<'c, 'i> RecipePicker<'c, 'i> {
    fn new(
        contents: &'c mut StackedItemContents,
        ingredients: &'i [GenericIngredient<'i>],
    ) -> Self {
        // `getUniqueAvailableIngredientItems`
        let items: Vec<&'static Item> = contents
            .amounts
            .iter()
            .filter(|(item, count)| {
                *count > 0
                    && ingredients
                        .iter()
                        .any(|ingredient| ingredient.match_item(item))
            })
            .map(|(item, _)| *item)
            .collect();
        let pairs = items.len() * ingredients.len();
        let mut picker = Self {
            contents,
            ingredients,
            visited_ingredients: vec![false; ingredients.len()],
            visited_items: vec![false; items.len()],
            satisfied: vec![false; ingredients.len()],
            connections: vec![false; pairs],
            assigned: vec![false; pairs],
            items,
            path: Vec::new(),
        };
        for (ingredient, info) in ingredients.iter().enumerate() {
            for item in 0..picker.items.len() {
                if info.match_item(picker.items[item]) {
                    let index = picker.pair(item, ingredient);
                    picker.connections[index] = true;
                }
            }
        }
        picker
    }

    const fn pair(&self, item: usize, ingredient: usize) -> usize {
        item * self.ingredients.len() + ingredient
    }

    fn is_assigned(&self, item: usize, ingredient: usize) -> bool {
        self.assigned[self.pair(item, ingredient)]
    }

    fn set_assigned(&mut self, item: usize, ingredient: usize, assigned: bool) {
        let index = self.pair(item, ingredient);
        self.assigned[index] = assigned;
    }

    fn clear_all_visited(&mut self) {
        self.visited_ingredients.fill(false);
        self.visited_items.fill(false);
    }

    fn try_pick(&mut self, capacity: u32, output: Option<&mut Vec<&'static Item>>) -> bool {
        if capacity == 0 {
            return true;
        }
        let mut satisfied_count = 0;
        loop {
            let Some(path) = self.try_assigning_new_item(capacity) else {
                let is_valid = satisfied_count == self.ingredients.len();
                let mut output = output.filter(|_| is_valid);
                self.clear_all_visited();
                self.satisfied.fill(false);
                for ingredient in 0..self.ingredients.len() {
                    for item in 0..self.items.len() {
                        if self.is_assigned(item, ingredient) {
                            self.set_assigned(item, ingredient, false);
                            self.contents.put(self.items[item], capacity);
                            if let Some(output) = output.as_deref_mut() {
                                output.push(self.items[item]);
                            }
                            break;
                        }
                    }
                }
                return is_valid;
            };

            self.contents.take(self.items[path[0]], capacity);
            self.satisfied[path[path.len() - 1]] = true;
            satisfied_count += 1;
            for i in 0..path.len() - 1 {
                if i.is_multiple_of(2) {
                    self.set_assigned(path[i], path[i + 1], true);
                } else {
                    self.set_assigned(path[i + 1], path[i], false);
                }
            }
        }
    }

    fn try_assigning_new_item(&mut self, capacity: u32) -> Option<Vec<usize>> {
        self.clear_all_visited();
        for item in 0..self.items.len() {
            if self.contents.amount(self.items[item]) >= capacity
                && let Some(path) = self.find_new_item_assignment_path(item)
            {
                return Some(path);
            }
        }
        None
    }

    /// The path alternates item, ingredient, item, ... indices, starting at `starting_item`.
    fn find_new_item_assignment_path(&mut self, starting_item: usize) -> Option<Vec<usize>> {
        self.path.clear();
        self.visited_items[starting_item] = true;
        self.path.push(starting_item);

        while let Some(&last) = self.path.last() {
            let path_length = self.path.len();
            if (path_length - 1).is_multiple_of(2) {
                let item_to_assign = last;
                for ingredient in 0..self.ingredients.len() {
                    if !self.visited_ingredients[ingredient]
                        && self.connections[self.pair(item_to_assign, ingredient)]
                        && !self.is_assigned(item_to_assign, ingredient)
                    {
                        self.visited_ingredients[ingredient] = true;
                        self.path.push(ingredient);
                        break;
                    }
                }
            } else {
                let last_assigned_ingredient = last;
                if !self.satisfied[last_assigned_ingredient] {
                    return Some(self.path.clone());
                }
                for item in 0..self.items.len() {
                    if !self.visited_items[item] && self.is_assigned(item, last_assigned_ingredient)
                    {
                        self.visited_items[item] = true;
                        self.path.push(item);
                        break;
                    }
                }
            }
            if self.path.len() == path_length {
                self.path.pop();
            }
        }
        None
    }

    fn try_pick_all(&mut self, max_size: u32, output: Option<&mut Vec<&'static Item>>) -> u32 {
        let mut min = 0u32;
        let mut max = max_size
            .min(self.contents.result_upper_bound(self.ingredients))
            .saturating_add(1);
        loop {
            let mid = min + (max - min) / 2;
            if self.try_pick(mid, None) {
                if max - min <= 1 {
                    if mid > 0 {
                        self.try_pick(mid, output);
                    }
                    return mid;
                }
                min = mid;
            } else {
                max = mid;
            }
        }
    }
}

/// `PlaceRecipeHelper.placeRecipe`: hands every recipe slot's entry to `output` with the grid
/// index it goes to, centering recipes smaller than half the grid.
pub fn place_recipe_slots(
    grid_width: usize,
    grid_height: usize,
    recipe_width: usize,
    recipe_height: usize,
    entries: &[Option<usize>],
    mut output: impl FnMut(Option<usize>, usize),
) {
    let mut iterator = entries.iter().peekable();
    let mut grid_index = 0;
    let mut grid_y = 0;
    while grid_y < grid_height {
        let center_rows = (recipe_height as f32) < grid_height as f32 / 2.0;
        let start_row = (grid_height as f32 / 2.0 - recipe_height as f32 / 2.0).floor() as usize;
        if center_rows && start_row > grid_y {
            grid_index += grid_width;
            grid_y += 1;
        }

        for grid_x in 0..grid_width {
            if iterator.peek().is_none() {
                return;
            }
            let center_columns = (recipe_width as f32) < grid_width as f32 / 2.0;
            let start_column =
                (grid_width as f32 / 2.0 - recipe_width as f32 / 2.0).floor() as usize;
            let (total_recipe_width, add_ingredient) = if center_columns {
                (
                    start_column + recipe_width,
                    start_column <= grid_x && grid_x < start_column + recipe_width,
                )
            } else {
                (recipe_width, grid_x < recipe_width)
            };

            if add_ingredient {
                if let Some(entry) = iterator.next() {
                    output(*entry, grid_index);
                }
            } else if total_recipe_width == grid_x {
                grid_index += grid_width - grid_x;
                break;
            }
            grid_index += 1;
        }
        grid_y += 1;
    }
}

/// `ServerPlaceRecipe.testClearGrid`: whether everything in the grid fits back into the
/// inventory.
#[must_use]
pub fn test_clear_grid(inventory: &PlayerInventory, grid: &[ItemStack]) -> bool {
    let free_slots_in_inventory = inventory
        .main_inventory
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .filter(|stack| stack.is_empty())
        .count();
    let mut free_slots: Vec<ItemStack> = Vec::new();
    for stack in grid {
        let mut stack = stack.clone();
        if stack.is_empty() {
            continue;
        }
        if inventory.get_occupied_slot_with_room_for_stack(&stack) != -1 {
            continue;
        }
        if free_slots.len() > free_slots_in_inventory {
            return false;
        }
        for other in &mut free_slots {
            if other.item.id == stack.item.id
                && other.item_count != other.get_max_stack_size()
                && u32::from(other.item_count) + u32::from(stack.item_count)
                    <= u32::from(other.get_max_stack_size())
            {
                other.item_count += stack.item_count;
                stack.item_count = 0;
                break;
            }
        }
        if !stack.is_empty() {
            if free_slots.len() >= free_slots_in_inventory {
                return false;
            }
            free_slots.push(stack);
        }
    }
    true
}

/// `Inventory.findSlotMatchingCraftingIngredient`.
#[must_use]
pub fn find_slot_matching_crafting_ingredient(
    main_inventory: &[ItemStack],
    item: &Item,
    existing: &ItemStack,
) -> Option<usize> {
    main_inventory.iter().position(|stack| {
        !stack.is_empty()
            && stack.item.id == item.id
            && is_usable_for_crafting(stack)
            && (existing.is_empty() || existing.are_items_and_components_equal(stack))
    })
}
