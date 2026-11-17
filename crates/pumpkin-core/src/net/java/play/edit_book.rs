#[allow(clippy::wildcard_imports)]
use super::*;

impl JavaClient {
    /// Vanilla `ServerGamePacketListenerImpl.handleEditBook`. Pumpkin has no text filter
    /// service, so the text passes through unfiltered like vanilla's `TextFilter.DUMMY`.
    pub fn handle_edit_book(&self, player: &Player, packet: &SEditBook<'_>) {
        let Ok(slot) = usize::try_from(packet.slot.0) else {
            return;
        };
        if !PlayerInventory::is_valid_hotbar_index(slot) && slot != PlayerInventory::OFF_HAND_SLOT {
            return;
        }
        let carried = player.inventory().get_slot(slot);
        if !carried.has_data_component(DataComponent::WritableBookContent) {
            return;
        }

        let mut pages: Vec<String> = packet.pages.iter().map(|p| (*p).to_string()).collect();
        let mut title = packet.title.map(std::string::ToString::to_string);
        let signing = title.is_some();

        if let Some(player_arc) = player.world().get_player_by_uuid(player.gameprofile.id)
            && let Some(server) = player.world().server.upgrade()
        {
            let mut event =
                crate::plugin::api::events::player::player_edit_book::PlayerEditBookEvent {
                    player: player_arc,
                    slot: slot as u32,
                    pages: pages.clone(),
                    title: title.clone(),
                    signing,
                    cancelled: false,
                };
            server.plugin_manager.fire_blocking(&server, &mut event);
            if event.cancelled {
                return;
            }
            pages = event.pages;
            title = event.title;
        }

        if let Some(title) = title {
            // `signBook`: keep the book's other components, swap the writable content for
            // the written content.
            let mut written_book = carried.transmute_copy(&Item::WRITTEN_BOOK, carried.item_count);
            // `remove` on a component the written book has no default for drops it from the
            // patch instead of recording a removal.
            written_book
                .patch
                .retain(|(component, _)| *component != DataComponent::WritableBookContent);
            written_book.set_data_component(WrittenBookContentImpl {
                title,
                author: player.gameprofile.name.clone(),
                pages: pages.into_iter().map(TextComponent::text).collect(),
                generation: 0,
                resolved: true,
            });
            player.inventory().set_slot(slot, written_book);
        } else {
            // `updateBookContents`
            let mut writable_book = carried;
            writable_book.set_data_component(WritableBookContentImpl { pages });
            player.inventory().set_slot(slot, writable_book);
        }
    }
}
