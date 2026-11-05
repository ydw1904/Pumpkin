#[allow(clippy::wildcard_imports)]
use super::*;

impl PendingConnection {
    pub async fn handle_known_packs(&mut self, server: &Server) {
        let synced = server.datapack_manager.get_all_synced_registries();
        for reg in &synced {
            self.send_packet_now(&CRegistryData::new(&reg.registry_id, &reg.registry_entries))
                .await;
        }
        let tags = crate::data::datapack::DatapackManager::network_tag_keys();
        let block_tags = server.datapack_manager.datapack_block_tags();
        self.send_packet_now(&CUpdateTags::new(&tags).with_block_tags(&block_tags))
            .await;
        self.send_packet_now(&CFinishConfig).await;
    }
}
