#[allow(clippy::wildcard_imports)]
use super::*;
use crate::entity::vehicle::vehicle::fire_vehicle_move;
use pumpkin_protocol::java::client::play::CMoveVehicle;

impl JavaClient {
    pub fn handle_move_vehicle(&self, player: &Arc<Player>, packet: &SMoveVehicle) {
        // A movement packet was received this tick — tracked for SClientTickEnd zeroing.
        self.received_movement_this_tick
            .store(true, Ordering::Relaxed);
        let entity = player.get_entity();
        let last_pos = entity.pos.load();
        let mut pos = Vector3::new(packet.x, packet.y, packet.z);
        let vehicle = entity
            .vehicle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(vehicle) = &vehicle {
            let vehicle_entity = vehicle.get_entity();
            let old_pos = vehicle_entity.pos.load();
            let old_y = old_pos.y;
            let allowed = fire_vehicle_move(vehicle_entity, old_pos, pos);
            if allowed != pos {
                // Like a rejected move: the client's vehicle goes back where the server has it.
                pos = allowed;
                self.try_send_packet(&CMoveVehicle::new(
                    pos.x,
                    pos.y,
                    pos.z,
                    packet.yaw,
                    packet.pitch,
                ));
            }
            vehicle_entity.set_pos(pos);
            vehicle_entity.set_rotation(packet.yaw, packet.pitch);
            vehicle_entity
                .on_ground
                .store(packet.on_ground, Ordering::Relaxed);
            vehicle_entity.emit_client_movement(vehicle.as_ref(), pos - old_pos);
            if let Some(living) = vehicle.get_living_entity() {
                living.fall(vehicle.as_ref(), pos.y - old_y, packet.on_ground);
            }

            let world = player.world();
            let vehicle_pos = pos.to_block_pos();
            let check_below = BlockPos::new(vehicle_pos.0.x, vehicle_pos.0.y - 1, vehicle_pos.0.z);
            let in_lava = world.get_block(&vehicle_pos) == &pumpkin_data::Block::LAVA
                || world.get_block(&check_below) == &pumpkin_data::Block::LAVA;

            if in_lava {
                let start_pos = player.entered_lava_pos.load().unwrap_or(pos);
                if player.entered_lava_pos.load().is_none() {
                    player.entered_lava_pos.store(Some(pos));
                }
                let dx = pos.x - start_pos.x;
                let dz = pos.z - start_pos.z;
                let distance_horizontal = dx.hypot(dz);
                player.trigger_advancement(
                    crate::entity::player::advancement::trigger::AdvancementTrigger::RideEntityInLava {
                        distance_horizontal,
                        dimension: world.dimension.minecraft_name.to_string(),
                        vehicle_type: Some(format!(
                            "minecraft:{}",
                            vehicle_entity.entity_type.resource_name
                        )),
                    },
                );
            } else {
                player.entered_lava_pos.store(None);
            }
        } else {
            player.entered_lava_pos.store(None);
        }
        match &vehicle {
            Some(vehicle) => crate::entity::position_rider(vehicle.as_ref(), player.as_ref()),
            None => entity.set_pos(pos),
        }
        let new_pos = entity.pos.load();
        let distance = last_pos.squared_distance_to_vec(&new_pos).sqrt();
        let cm = (distance * 100.0).round() as i32;
        if cm > 0 {
            let stat = player.get_movement_statistic();
            player.increment_stat(
                pumpkin_data::statistic::StatisticCategory::Custom,
                stat as i32,
                cm,
            );
        }
        chunker::update_position(player);
    }
}
