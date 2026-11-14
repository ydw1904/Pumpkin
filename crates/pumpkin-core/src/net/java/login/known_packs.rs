#[allow(clippy::wildcard_imports)]
use super::*;

impl PendingConnection {
    /// Registries and tags are sent in the 26.3 format; for older clients the multiversion
    /// plugin rewrites them in `JavaConnectionPacketSentEvent`.
    pub async fn handle_known_packs(&mut self, server: &Server) {
        let synced = server.datapack_manager.get_all_synced_registries();
        for reg in &synced {
            self.send_packet_now(&CRegistryData::new(&reg.registry_id, &reg.registry_entries))
                .await;
        }
        let tags = server.datapack_manager.network_tags();
        self.send_packet_now(&CUpdateTags::new(&tags)).await;
        self.send_packet_now(&CFinishConfig).await;
    }
}
