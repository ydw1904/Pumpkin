//! Server side of the Servux `servux:hud_metadata` plugin channel.
//!
//! Malilib-based clients (`MiniHUD`) open this channel to read world data the
//! vanilla protocol never sends: the world spawn point and, when the server
//! allows it, the world seed. Vanilla clients never open the channel, so this
//! is inert for them.
//!
//! The wire format follows Servux `LTS/1.21.11`: a `VarInt` packet type
//! followed by a body whose encoding depends on the type. The two metadata
//! packets carry plain vanilla network NBT; every other packet carries a
//! big-endian `i32` byte length followed by a gzip-compressed NBT compound
//! with an empty root name.

use std::io::Cursor;

use pumpkin_data::dimension::Dimension;
use pumpkin_nbt::{Nbt, NbtCompound, deserializer::NbtReadHelperJava, nbt_compress};
use pumpkin_protocol::{
    codec::var_int::VarInt,
    ser::{NetworkReadExt, NetworkWriteExt},
};
use tracing::debug;

use crate::{entity::player::Player, server::Server};

/// The plugin channel `MiniHUD` opens for server-provided world data.
pub const CHANNEL: &str = "servux:hud_metadata";

/// Protocol version this channel speaks.
///
/// Servux refuses clients advertising a lower version than the server's, so
/// this must track `ServuxHudPacket.PROTOCOL_VERSION`.
const PROTOCOL_VERSION: i32 = 3;

/// The provider name Servux reports for this channel.
const PROVIDER_NAME: &str = "hud_data";

// Packet types, from `ServuxHudPacket.Type`. Only the ones we act on are named.
const S2C_METADATA: i32 = 1;
const C2S_METADATA_REQUEST: i32 = 2;
const S2C_SPAWN_DATA: i32 = 3;
const C2S_SPAWN_DATA_REQUEST: i32 = 4;
const C2S_UNREGISTER_REPLY: i32 = 9;

/// Handles one payload received on [`CHANNEL`].
pub async fn handle_payload(server: &Server, player: &Player, mut data: &[u8]) {
    let config = &server.advanced_config.servux;
    if !config.enabled {
        return;
    }

    let Ok(packet_type) = data.get_var_int() else {
        debug!("servux: received a payload with no packet type");
        return;
    };

    match packet_type.0 {
        C2S_METADATA_REQUEST => {
            // Mirror Servux: refuse clients older than the server's protocol
            // rather than replying with fields they cannot parse.
            let client_version = client_protocol_version(data);
            if client_version < PROTOCOL_VERSION {
                debug!(
                    "servux: denying {}, client protocol {client_version} < {PROTOCOL_VERSION}",
                    player.gameprofile.name
                );
                return;
            }
            send_metadata(server, player, config.share_seed).await;
        }
        C2S_SPAWN_DATA_REQUEST => send_spawn_data(server, player, config.share_seed).await,
        C2S_UNREGISTER_REPLY => {
            // Nothing to drop: replies are computed per request, not pushed.
            debug!("servux: {} unregistered", player.gameprofile.name);
        }
        other => debug!("servux: ignoring unhandled packet type {other}"),
    }
}

/// Reads the `version` field from a metadata request body.
///
/// Returns `-1` when the body is missing or unreadable, matching the default
/// Servux uses so that a malformed request is treated as too old.
fn client_protocol_version(data: &[u8]) -> i32 {
    let mut reader = NbtReadHelperJava::new(Cursor::new(data));
    Nbt::read_unnamed(&mut reader)
        .ok()
        .and_then(|nbt| nbt.get_int("version"))
        .unwrap_or(-1)
}

/// Sends the metadata handshake reply that registers the client.
async fn send_metadata(server: &Server, player: &Player, share_seed: bool) {
    let mut nbt = NbtCompound::new();
    nbt.put_string("name", PROVIDER_NAME.to_string());
    nbt.put_string("id", CHANNEL.to_string());
    nbt.put_int("version", PROTOCOL_VERSION);
    nbt.put_string(
        "servux",
        format!("servux-pumpkin-{}", env!("CARGO_PKG_VERSION")),
    );
    put_spawn_data(server, &mut nbt, share_seed);

    send(player, S2C_METADATA, &Nbt::new(String::new(), nbt).write_unnamed()).await;
}

/// Sends the spawn/seed payload on demand.
async fn send_spawn_data(server: &Server, player: &Player, share_seed: bool) {
    let mut nbt = NbtCompound::new();
    put_spawn_data(server, &mut nbt, share_seed);

    let Ok(compressed) = nbt_compress::write_gzip_compound_tag_to_bytes(nbt) else {
        debug!("servux: failed to compress spawn data");
        return;
    };

    // The gzip body is length-prefixed with a big-endian i32, not a VarInt.
    let Ok(length) = i32::try_from(compressed.len()) else {
        debug!("servux: spawn data too large to length-prefix");
        return;
    };

    let mut body = Vec::with_capacity(compressed.len() + 4);
    if body.write_i32_be(length).is_err() || body.write_slice(&compressed).is_err() {
        return;
    }

    send(player, S2C_SPAWN_DATA, &body).await;
}

/// Writes the world spawn fields, and the seed when the server shares it.
///
/// The world spawn is a property of the overworld in vanilla, so it is read
/// from the overworld regardless of which world the player is currently in.
fn put_spawn_data(server: &Server, nbt: &mut NbtCompound, share_seed: bool) {
    let overworld = server.get_world_from_dimension(&Dimension::OVERWORLD);
    let level_info = overworld.level_info.load();

    nbt.put_string(
        "spawnDimension",
        Dimension::OVERWORLD.minecraft_name.to_string(),
    );
    nbt.put_int("spawnPosX", level_info.spawn_x);
    nbt.put_int("spawnPosY", level_info.spawn_y);
    nbt.put_int("spawnPosZ", level_info.spawn_z);

    if share_seed {
        nbt.put_long("worldSeed", overworld.level.seed.0.cast_signed());
    }
}

/// Prefixes `body` with its packet type and sends it on [`CHANNEL`].
async fn send(player: &Player, packet_type: i32, body: &[u8]) {
    let Some(java) = player.as_java() else {
        return;
    };

    let mut payload = Vec::with_capacity(body.len() + 1);
    if payload.write_var_int(&VarInt(packet_type)).is_err()
        || payload.write_slice(body).is_err()
    {
        return;
    }

    java.send_custom_payload(CHANNEL, &payload).await;
}
