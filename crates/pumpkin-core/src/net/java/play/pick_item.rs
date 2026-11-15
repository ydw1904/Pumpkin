#[allow(clippy::wildcard_imports)]
use super::*;
use pumpkin_data::tag::Taggable;

impl JavaClient {
    pub fn handle_pick_item_from_block(
        &self,
        player: &Arc<Player>,
        pick_item: &SPickItemFromBlock,
    ) {
        if !player.can_interact_with_block_at(&pick_item.pos, 1.0) {
            return;
        }

        let world = player.world();
        let block = world.get_block(&pick_item.pos);

        if block.item_id == 0 {
            // Invalid block id (blocks such as tall seagrass)
            return;
        }

        let Some(item) = Item::from_id(block.item_id) else {
            return;
        };
        let include_data = player.is_creative() && pick_item.include_data;
        let state_id = world.get_block_state_id(&pick_item.pos);
        let block_entity = world.get_block_entity(&pick_item.pos);
        let mut stack = ItemStack::new(1, item);
        // The `getCloneItemStack` overrides that read the block entity.
        if let Some(block_entity) = &block_entity {
            if let Some(statue) = block_entity.as_any().downcast_ref::<
                crate::block::entities::copper_golem_statue::CopperGolemStatueBlockEntity,
            >() {
                let props =
                    pumpkin_data::block_properties::CopperGolemStatueLikeProperties::from_state_id(
                        state_id,
                    );
                statue.get_item(&mut stack, props.copper_golem_pose);
            } else if block_entity
                .as_any()
                .is::<crate::block::entities::banner::BannerBlockEntity>()
            {
                // `BannerBlockEntity.getItem`
                world.collect_block_entity_components(block_entity.as_ref(), &mut stack);
            } else if block_entity
                .as_any()
                .is::<crate::block::entities::decorated_pot::DecoratedPotBlockEntity>()
            {
                // `DecoratedPotBlockEntity.createDecoratedPotInstance`
                let mut collected = ItemStack::new(1, item);
                block_entity.collect_item_components(&mut collected);
                if let Some(decorations) = collected
                    .get_data_component::<pumpkin_data::data_component_impl::PotDecorationsImpl>()
                {
                    stack.set_data_component(decorations.clone());
                }
            }
        }
        if include_data
            && block.has_tag(&pumpkin_data::tag::Block::MINECRAFT_BEEHIVES)
            && let Some(props) = block.properties(state_id)
        {
            // `BeehiveBlock.getCloneItemStack`
            stack.set_data_component(pumpkin_data::data_component_impl::BlockStateImpl {
                properties: std::borrow::Cow::Owned(
                    props
                        .to_props()
                        .into_iter()
                        .filter(|property| property.key().to_str() == "honey_level")
                        .map(|property| {
                            (
                                std::borrow::Cow::Borrowed("honey_level"),
                                std::borrow::Cow::Owned(property.value_str().to_owned()),
                            )
                        })
                        .collect(),
                ),
            });
        }
        if include_data && let Some(block_entity) = &block_entity {
            Self::add_block_data_to_item(&world, block_entity.as_ref(), &mut stack);
        }

        let slot_with_stack = player.inventory().get_slot_with_stack(&stack);

        if slot_with_stack != -1 {
            if PlayerInventory::is_valid_hotbar_index(slot_with_stack as usize) {
                player.inventory.set_selected_slot(slot_with_stack as u8);
            } else {
                player
                    .inventory
                    .swap_slot_with_hotbar(slot_with_stack as usize);
            }
        } else if player.gamemode.load() == GameMode::Creative {
            player.inventory.swap_stack_with_hotbar(stack);
        }

        player.try_send_client_packet(&CSetSelectedSlot::new(
            player.inventory.get_selected_slot() as i8
        ));
        player
            .player_screen_handler
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .send_content_updates();
    }

    /// Vanilla `addBlockDataToItem`: the block entity's data without the fields its components
    /// carry, then its components.
    fn add_block_data_to_item(
        world: &crate::world::World,
        block_entity: &dyn crate::block::entities::BlockEntity,
        stack: &mut ItemStack,
    ) {
        let mut nbt = world.block_entity_data(block_entity);
        let id = block_entity.resource_location();
        for field in ["id", "x", "y", "z", "components"].into_iter().chain(
            crate::block::entities::block_entity_component_fields(id)
                .iter()
                .copied(),
        ) {
            nbt.child_tags.remove(field);
        }
        // `BlockItem.setBlockEntityData` leaves the component out when nothing is left.
        if !nbt.is_empty() {
            nbt.put_string("id", id.to_string());
            stack
                .set_data_component(pumpkin_data::data_component_impl::BlockEntityDataImpl { nbt });
        }
        world.collect_block_entity_components(block_entity, stack);
    }

    pub fn handle_pick_item_from_entity(
        &self,
        player: &Arc<Player>,
        pick_item: &SPickItemFromEntity,
    ) {
        use pumpkin_data::entity::{entity_from_egg, spawn_egg_ids};

        let world = player.world();
        let Some(target) = world.get_entity_by_id(pick_item.id.0) else {
            return;
        };

        let p_eye = player.get_entity().get_eye_pos();
        let t_eye = target.get_eye_pos();
        let dx = p_eye.x - t_eye.x;
        let dy = p_eye.y - t_eye.y;
        let dz = p_eye.z - t_eye.z;
        if dx * dx + dy * dy + dz * dz > 64.0 {
            return;
        }

        let target_type_id = target.get_entity().entity_type.id;
        let mut found_egg: Option<u16> = None;
        for &egg_id in &spawn_egg_ids() {
            if let Some(et) = entity_from_egg(egg_id)
                && et.id == target_type_id
            {
                found_egg = Some(egg_id);
                break;
            }
        }

        if let Some(item) = found_egg.and_then(Item::from_id) {
            let stack = ItemStack::new(1, item);

            let slot_with_stack = player.inventory().get_slot_with_stack(&stack);

            if slot_with_stack != -1 {
                if PlayerInventory::is_valid_hotbar_index(slot_with_stack as usize) {
                    player.inventory.set_selected_slot(slot_with_stack as u8);
                } else {
                    player
                        .inventory
                        .swap_slot_with_hotbar(slot_with_stack as usize);
                }
            } else if player.gamemode.load() == GameMode::Creative {
                player.inventory.swap_stack_with_hotbar(stack);
            }

            player.try_send_client_packet(&CSetSelectedSlot::new(
                player.inventory.get_selected_slot() as i8,
            ));
            player
                .player_screen_handler
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .send_content_updates();
        }
    }
}
