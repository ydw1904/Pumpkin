use std::sync::Arc;

use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::packet::CURRENT_MC_VERSION;
use pumpkin_data::particle::Particle;
use pumpkin_data::translation;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_protocol::codec::item_stack_seralizer::ItemStackSerializer;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::java::client::play::CParticle;
use pumpkin_protocol::ser::NetworkWriteExt;
use pumpkin_util::PermissionLvl;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::permission::{Permission, PermissionDefault, PermissionRegistry};
use pumpkin_util::text::TextComponent;
use pumpkin_world::generation::structure::template::PaletteEntry;

use crate::command::argument_builder::{ArgumentBuilder, argument, command, literal};
use crate::command::argument_types::coordinates::vec3::Vec3ArgumentType;
use crate::command::argument_types::core::float::FloatArgumentType;
use crate::command::argument_types::core::integer::IntegerArgumentType;
use crate::command::argument_types::entity::EntityArgumentType;
use crate::command::argument_types::particle::{
    ERROR_INVALID_OPTIONS, ParticleArgumentType, ParticleWithOptions,
};
use crate::command::commands::data::snbt_string;
use crate::command::context::command_context::CommandContext;
use crate::command::context::command_source::CommandSource;
use crate::command::errors::command_syntax_error::CommandSyntaxError;
use crate::command::errors::error_types::CommandErrorType;
use crate::command::node::dispatcher::CommandDispatcher;
use crate::command::node::{CommandExecutor, CommandExecutorResult};
use crate::entity::player::Player;

const DESCRIPTION: &str = "Creates particles in the world.";
const PERMISSION: &str = "minecraft:command.particle";

const ERROR_FAILED: CommandErrorType<0> = CommandErrorType::new(
    translation::java::COMMANDS_PARTICLE_FAILED,
    translation::java::COMMANDS_PARTICLE_FAILED,
);

#[allow(clippy::too_many_arguments)]
fn send_particles(
    source: &CommandSource,
    particle: &ParticleWithOptions,
    pos: Vector3<f64>,
    delta: Vector3<f64>,
    speed: f32,
    count: i32,
    force: bool,
    viewers: Option<Vec<Arc<Player>>>,
) -> Result<i32, CommandSyntaxError> {
    let data =
        encode_particle_options(particle.particle, &particle.options).map_err(|message| {
            ERROR_INVALID_OPTIONS.create_without_context(TextComponent::text(message))
        })?;
    let particle = particle.particle;
    let world = source.world().clone();
    let players: Vec<Arc<Player>> = viewers.unwrap_or_else(|| world.players.load().to_vec());

    let offset = Vector3::new(delta.x as f32, delta.y as f32, delta.z as f32);
    let packet = CParticle::new(
        force,
        false,
        pos,
        offset,
        speed,
        count,
        VarInt(i32::from(particle.to_id())),
        &data,
    );

    let max_dist_sq = if force { 512.0 * 512.0 } else { 32.0 * 32.0 };
    let mut sent_count = 0;

    for player in &players {
        if !Arc::ptr_eq(&player.world(), &world) {
            continue;
        }
        let player_pos = player.position();
        let dx = player_pos.x - pos.x;
        let dy = player_pos.y - pos.y;
        let dz = player_pos.z - pos.z;
        let dist_sq = dx * dx + dy * dy + dz * dz;
        if dist_sq <= max_dist_sq {
            player.try_send_client_packet(&packet);
            sent_count += 1;
        }
    }

    if sent_count == 0 {
        return Err(ERROR_FAILED.create_without_context());
    }

    let particle_name = format!("minecraft:{}", particle.to_name());
    source.send_feedback(
        TextComponent::translate_cross(
            translation::java::COMMANDS_PARTICLE_SUCCESS,
            translation::java::COMMANDS_PARTICLE_SUCCESS,
            [TextComponent::text(particle_name)],
        ),
        true,
    );

    Ok(sent_count)
}

#[derive(Clone, Copy)]
enum ParticleMode {
    Simple,
    WithPos,
    Full { force: bool, has_viewers: bool },
}

struct ParticleExecutor {
    mode: ParticleMode,
}

impl CommandExecutor for ParticleExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let particle = ParticleArgumentType::get(context, "name")?;

        match self.mode {
            ParticleMode::Simple => {
                let pos = context.source.position;
                send_particles(
                    &context.source,
                    &particle,
                    pos,
                    Vector3::new(0.0, 0.0, 0.0),
                    0.0,
                    0,
                    false,
                    None,
                )
            }
            ParticleMode::WithPos => {
                let pos = Vec3ArgumentType::get_vector3(context, "pos")?;
                send_particles(
                    &context.source,
                    &particle,
                    pos,
                    Vector3::new(0.0, 0.0, 0.0),
                    0.0,
                    0,
                    false,
                    None,
                )
            }
            ParticleMode::Full { force, has_viewers } => {
                let pos = Vec3ArgumentType::get_vector3(context, "pos")?;
                let delta = Vec3ArgumentType::get_vector3(context, "delta")?;
                let speed = FloatArgumentType::get(context, "speed")?;
                let count = IntegerArgumentType::get(context, "count")?;
                let viewers = if has_viewers {
                    Some(EntityArgumentType::get_players(context, "viewers")?)
                } else {
                    None
                };

                send_particles(
                    &context.source,
                    &particle,
                    pos,
                    delta,
                    speed,
                    count,
                    force,
                    viewers,
                )
            }
        }
    }
}

pub fn register(dispatcher: &mut CommandDispatcher, registry: &PermissionRegistry) {
    registry.register_permission_or_panic(Permission::new(
        PERMISSION,
        DESCRIPTION,
        PermissionDefault::Op(PermissionLvl::Two),
    ));

    dispatcher.register(
        command("particle", DESCRIPTION).requires(PERMISSION).then(
            argument("name", ParticleArgumentType)
                .executes(ParticleExecutor {
                    mode: ParticleMode::Simple,
                })
                .then(
                    argument("pos", Vec3ArgumentType::Default)
                        .executes(ParticleExecutor {
                            mode: ParticleMode::WithPos,
                        })
                        .then(
                            argument("delta", Vec3ArgumentType::Uncorrected).then(
                                argument("speed", FloatArgumentType::with_min(0.0)).then(
                                    argument("count", IntegerArgumentType::with_min(0))
                                        .executes(ParticleExecutor {
                                            mode: ParticleMode::Full {
                                                force: false,
                                                has_viewers: false,
                                            },
                                        })
                                        .then(
                                            literal("force")
                                                .executes(ParticleExecutor {
                                                    mode: ParticleMode::Full {
                                                        force: true,
                                                        has_viewers: false,
                                                    },
                                                })
                                                .then(
                                                    argument(
                                                        "viewers",
                                                        EntityArgumentType::Players,
                                                    )
                                                    .executes(ParticleExecutor {
                                                        mode: ParticleMode::Full {
                                                            force: true,
                                                            has_viewers: true,
                                                        },
                                                    }),
                                                ),
                                        )
                                        .then(
                                            literal("normal")
                                                .executes(ParticleExecutor {
                                                    mode: ParticleMode::Full {
                                                        force: false,
                                                        has_viewers: false,
                                                    },
                                                })
                                                .then(
                                                    argument(
                                                        "viewers",
                                                        EntityArgumentType::Players,
                                                    )
                                                    .executes(ParticleExecutor {
                                                        mode: ParticleMode::Full {
                                                            force: false,
                                                            has_viewers: true,
                                                        },
                                                    }),
                                                ),
                                        ),
                                ),
                            ),
                        ),
                ),
        ),
    );
}

/// Encodes a particle's options for `CParticle` like its vanilla `ParticleType` codec and stream
/// codec. `Err` holds the reason the options don't decode (vanilla's `particle.invalidOptions`).
#[expect(
    clippy::too_many_lines,
    reason = "one arm per vanilla particle option type"
)]
fn encode_particle_options(particle: Particle, options: &NbtCompound) -> Result<Vec<u8>, String> {
    let mut data = Vec::new();
    // DFU's `MapLike` names the whole options compound.
    let missing = |name: &str| {
        format!(
            "No key {name} in MapLike[{}]",
            snbt_string(&NbtTag::Compound(options.clone()))
        )
    };
    let required = |name: &str| options.get(name).ok_or_else(|| missing(name));
    let written = |result: Result<(), pumpkin_protocol::ser::WritingError>| {
        result.map_err(|error| error.to_string())
    };
    match particle {
        // `DustParticleOptions`.
        Particle::Dust => {
            // A record codec reports every missing field, the later one first.
            if options.get("color").is_none() && options.get("scale").is_none() {
                return Err(format!("{}; {}", missing("scale"), missing("color")));
            }
            let color = rgb_color(required("color")?)?;
            let scale = scale(required("scale")?)?;
            written(
                data.write_i32_be(color)
                    .and_then(|()| data.write_f32_be(scale)),
            )?;
        }
        // `DustColorTransitionOptions`.
        Particle::DustColorTransition => {
            let from = rgb_color(required("from_color")?)?;
            let to = rgb_color(required("to_color")?)?;
            let scale = scale(required("scale")?)?;
            written(
                data.write_i32_be(from)
                    .and_then(|()| data.write_i32_be(to))
                    .and_then(|()| data.write_f32_be(scale)),
            )?;
        }
        // `SpellParticleOption`.
        Particle::Effect | Particle::InstantEffect => {
            let color = options.get("color").map_or(Ok(-1), rgb_color)?;
            let power = options.get("power").map_or(Ok(1.0), float)?;
            written(
                data.write_i32_be(color)
                    .and_then(|()| data.write_f32_be(power)),
            )?;
        }
        // `ColorParticleOption`.
        Particle::EntityEffect | Particle::TintedLeaves | Particle::Flash => {
            let color = argb_color(required("color")?)?;
            written(data.write_i32_be(color))?;
        }
        // `PowerParticleOption`.
        Particle::DragonBreath => {
            let power = options.get("power").map_or(Ok(1.0), float)?;
            written(data.write_f32_be(power))?;
        }
        // `BlockParticleOption`.
        Particle::Block
        | Particle::BlockMarker
        | Particle::FallingDust
        | Particle::DustPillar
        | Particle::BlockCrumble => {
            let state = PaletteEntry::state_from_nbt(required("block_state")?)
                .ok_or_else(|| "Unknown block state".to_string())?;
            written(data.write_var_int(&VarInt(i32::from(state.as_u16()))))?;
        }
        // `ItemParticleOption`: an `ItemStackTemplate`.
        Particle::Item => {
            let stack = item_template(required("item")?)?;
            written(
                ItemStackSerializer(std::borrow::Cow::Owned(stack))
                    .write_template_with_version(&mut data, &CURRENT_MC_VERSION),
            )?;
        }
        // `SculkChargeParticleOptions`.
        Particle::SculkCharge => {
            let roll = float(required("roll")?)?;
            written(data.write_f32_be(roll))?;
        }
        // `ShriekParticleOption`.
        Particle::Shriek => {
            let delay = int(required("delay")?)?;
            written(data.write_var_int(&VarInt(delay)))?;
        }
        // `TrailParticleOption`.
        Particle::Trail => {
            let target = vec3(required("target")?)?;
            let color = rgb_color(required("color")?)?;
            let duration = positive_int(required("duration")?)?;
            written(
                data.write_f64_be(target[0])
                    .and_then(|()| data.write_f64_be(target[1]))
                    .and_then(|()| data.write_f64_be(target[2]))
                    .and_then(|()| data.write_i32_be(color))
                    .and_then(|()| data.write_var_int(&VarInt(duration))),
            )?;
        }
        // `VibrationParticleOption`; only block destinations are supported.
        Particle::Vibration => {
            let NbtTag::Compound(destination) = required("destination")? else {
                return Err("Not a map".to_string());
            };
            let arrival = int(required("arrival_in_ticks")?)?;
            let kind = destination.get_string("type").unwrap_or_default();
            if kind.strip_prefix("minecraft:").unwrap_or(kind) != "block" {
                return Err(format!("Unsupported position source {kind}"));
            }
            let pos: Vec<i32> = match destination.get("pos") {
                Some(NbtTag::IntArray(pos)) => pos.clone(),
                Some(NbtTag::List(pos)) => pos.iter().map(int).collect::<Result<_, _>>()?,
                _ => {
                    return Err(format!(
                        "No key pos in MapLike[{}]",
                        snbt_string(&NbtTag::Compound(destination.clone()))
                    ));
                }
            };
            let [x, y, z] = pos[..] else {
                return Err(format!("Input is not a list of 3 elements: {pos:?}"));
            };
            let pos = BlockPos(Vector3::new(x, y, z));
            written(
                data.write_var_int(&VarInt(BLOCK_POSITION_SOURCE))
                    .and_then(|()| data.write_i64_be(pos.as_long()))
                    .and_then(|()| data.write_var_int(&VarInt(arrival))),
            )?;
        }
        // `GeyserParticleOptions`.
        Particle::Geyser | Particle::GeyserPlume => {
            let water_blocks = positive_int(required("water_blocks")?)?;
            written(data.write_i32_be(water_blocks))?;
        }
        // `GeyserBaseParticleOptions`.
        Particle::GeyserBase | Particle::GeyserPoof => {
            let water_blocks = positive_int(required("water_blocks")?)?;
            let impulse = float(required("burst_impulse_base")?)?;
            written(
                data.write_i32_be(water_blocks)
                    .and_then(|()| data.write_f32_be(impulse)),
            )?;
        }
        _ => {}
    }
    Ok(data)
}

/// The registry id of `minecraft:block` in `POSITION_SOURCE_TYPE`.
const BLOCK_POSITION_SOURCE: i32 = 0;

fn float(tag: &NbtTag) -> Result<f32, String> {
    tag.as_numeric_float()
        .ok_or_else(|| format!("Not a number: {tag}"))
}

fn int(tag: &NbtTag) -> Result<i32, String> {
    tag.as_numeric_int()
        .ok_or_else(|| format!("Not a number: {tag}"))
}

/// `ExtraCodecs.POSITIVE_INT`.
fn positive_int(tag: &NbtTag) -> Result<i32, String> {
    let value = int(tag)?;
    if value > 0 {
        Ok(value)
    } else {
        Err(format!("Value must be positive: {value}"))
    }
}

/// `ScalableParticleOptionsBase.SCALE`: a float in `0.01..=4.0`.
fn scale(tag: &NbtTag) -> Result<f32, String> {
    let value = float(tag)?;
    if (MIN_PARTICLE_SCALE..=MAX_PARTICLE_SCALE).contains(&value) {
        Ok(value)
    } else {
        Err(format!(
            "Value must be within range [{MIN_PARTICLE_SCALE};{MAX_PARTICLE_SCALE:.1}]: {value:?}"
        ))
    }
}

const MIN_PARTICLE_SCALE: f32 = 0.01;
const MAX_PARTICLE_SCALE: f32 = 4.0;

fn floats<const N: usize>(tag: &NbtTag) -> Result<[f32; N], String> {
    let NbtTag::List(list) = tag else {
        return Err(format!("Not a list: {tag}"));
    };
    let values: Vec<f32> = list.iter().map(float).collect::<Result<_, _>>()?;
    values
        .try_into()
        .map_err(|values: Vec<f32>| format!("Input is not a list of {N} elements: {values:?}"))
}

fn vec3(tag: &NbtTag) -> Result<[f64; 3], String> {
    let NbtTag::List(list) = tag else {
        return Err(format!("Not a list: {tag}"));
    };
    let values: Vec<f64> = list
        .iter()
        .map(|value| {
            value
                .as_numeric_double()
                .ok_or_else(|| format!("Not a number: {value}"))
        })
        .collect::<Result<_, _>>()?;
    values
        .try_into()
        .map_err(|values: Vec<f64>| format!("Input is not a list of 3 elements: {values:?}"))
}

/// `ARGB.as8BitChannel`.
fn channel(value: f32) -> i32 {
    (value * 255.0).floor() as i32
}

/// `ARGB.color`.
const fn argb(alpha: i32, red: i32, green: i32, blue: i32) -> i32 {
    (alpha & 0xFF) << 24 | (red & 0xFF) << 16 | (green & 0xFF) << 8 | blue & 0xFF
}

/// `ExtraCodecs.RGB_COLOR_CODEC`: an int, or `[r, g, b]` floats made opaque.
fn rgb_color(tag: &NbtTag) -> Result<i32, String> {
    if let Some(color) = tag
        .as_numeric_int()
        .filter(|_| !matches!(tag, NbtTag::List(_)))
    {
        return Ok(color);
    }
    let [red, green, blue] = floats::<3>(tag)?;
    Ok(argb(255, channel(red), channel(green), channel(blue)))
}

/// `ExtraCodecs.ARGB_COLOR_CODEC`: an int, or `[r, g, b, a]` floats.
fn argb_color(tag: &NbtTag) -> Result<i32, String> {
    if let Some(color) = tag
        .as_numeric_int()
        .filter(|_| !matches!(tag, NbtTag::List(_)))
    {
        return Ok(color);
    }
    let [red, green, blue, alpha] = floats::<4>(tag)?;
    Ok(argb(
        channel(alpha),
        channel(red),
        channel(green),
        channel(blue),
    ))
}

/// `ItemStackTemplate.CODEC`: an item id, or `{id, count, components}` with a count of 1 to 99.
fn item_template(tag: &NbtTag) -> Result<ItemStack, String> {
    let compound = match tag {
        NbtTag::String(id) => {
            let mut compound = NbtCompound::new();
            compound.put_string("id", id.to_string());
            compound
        }
        NbtTag::Compound(compound) => compound.clone(),
        _ => return Err(format!("Not a map: {tag}")),
    };
    if let Some(count) = compound.get("count").and_then(NbtTag::as_numeric_int)
        && !(1..=MAX_TEMPLATE_COUNT).contains(&count)
    {
        return Err(format!(
            "Value {count} outside of range [1:{MAX_TEMPLATE_COUNT}]"
        ));
    }
    ItemStack::read_item_stack(&compound)
        .filter(|stack| !stack.is_empty())
        .ok_or_else(|| "Unknown item".to_string())
}

const MAX_TEMPLATE_COUNT: i32 = 99;
