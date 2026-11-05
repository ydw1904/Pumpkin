use crate::argument_types::FromStringReader;
use crate::argument_types::argument_type::{ArgumentType, JavaClientArgumentType};
use crate::context::command_context::CommandContext;
use crate::errors::command_syntax_error::CommandSyntaxError;
use crate::errors::error_types::CommandErrorType;
use crate::snbt::SnbtParser;
use crate::string_reader::StringReader;
use crate::suggestion::suggestions::{Suggestions, SuggestionsBuilder};
use pumpkin_data::particle::Particle;
use pumpkin_data::translation;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_util::identifier::Identifier;
use pumpkin_util::text::TextComponent;

pub const ERROR_UNKNOWN_PARTICLE: CommandErrorType<1> = CommandErrorType::new(
    translation::java::PARTICLE_NOTFOUND,
    translation::bedrock::COMMANDS_PARTICLE_NOTFOUND,
);

pub const ERROR_INVALID_OPTIONS: CommandErrorType<1> = CommandErrorType::new(
    translation::java::PARTICLE_INVALIDOPTIONS,
    translation::java::PARTICLE_INVALIDOPTIONS,
);

/// A particle and the SNBT options written after it, like `dust{color:[1,0,0],scale:2}`.
#[derive(Clone, Debug)]
pub struct ParticleWithOptions {
    pub particle: Particle,
    /// Vanilla reads an empty compound when no options are given.
    pub options: NbtCompound,
}

pub struct ParticleArgumentType;

impl<S: crate::source::CommandSource> ArgumentType<S> for ParticleArgumentType {
    type Item = ParticleWithOptions;

    /// Vanilla `ParticleArgument.readParticle`.
    fn parse(&self, reader: &mut StringReader) -> Result<Self::Item, CommandSyntaxError> {
        let identifier = Identifier::from_reader(reader)?;
        let particle = Particle::from_name(identifier.path())
            .or_else(|| Particle::from_name(&identifier.to_string()))
            .ok_or_else(|| {
                ERROR_UNKNOWN_PARTICLE.create(reader, TextComponent::text(identifier.to_string()))
            })?;
        let options = if reader.peek() == Some('{') {
            match SnbtParser::parse_for_commands(reader)? {
                NbtTag::Compound(options) => options,
                tag => {
                    return Err(ERROR_INVALID_OPTIONS
                        .create(reader, TextComponent::text(format!("Not a map: {tag}"))));
                }
            }
        } else {
            NbtCompound::new()
        };
        Ok(ParticleWithOptions { particle, options })
    }

    fn client_side_parser(&self) -> JavaClientArgumentType {
        JavaClientArgumentType::Particle
    }

    fn examples(&self) -> Vec<String> {
        vec![
            "foo".to_string(),
            "foo:bar".to_string(),
            "particle".to_string(),
        ]
    }

    fn list_suggestions(
        &self,
        _context: &CommandContext<S>,
        builder: SuggestionsBuilder,
    ) -> Suggestions {
        let particles = (0..=124u16)
            .filter_map(Particle::from_id)
            .map(|p| format!("{p:?}").to_lowercase())
            .collect();
        builder.filter_and_suggest_lowercase(particles).build()
    }
}

impl ParticleArgumentType {
    pub fn get<S: crate::source::CommandSource>(
        context: &CommandContext<S>,
        name: &str,
    ) -> Result<ParticleWithOptions, CommandSyntaxError> {
        Ok(context.get_argument::<ParticleWithOptions>(name)?.clone())
    }
}
