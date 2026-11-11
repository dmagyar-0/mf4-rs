use thiserror::Error;

#[derive(Debug, Error)]
pub enum MdfError {
    /// No channel with that name (optionally: in that group) exists.
    ///
    /// Raised by every data operation (`read`, `values`, `time_range`,
    /// `conversion`, `byte_ranges`). Metadata lookups (`group`, `channel`)
    /// return `None` instead.
    #[error("{}", channel_not_found_message(.name, .group))]
    ChannelNotFound {
        name: String,
        group: Option<String>,
    },

    /// A `ReadOptions` value (or another argument) is not usable.
    #[error("Invalid argument: {0}")]
    InvalidArgument(String),

    #[error("Buffer too small at {file}:{line}: need at least {expected} bytes, got {actual}")]
    TooShortBuffer {
        actual:   usize,
        expected: usize,
        file:     &'static str,
        line:     u32,
    },

    #[error(r#"Invalid file identifier: Expected "MDF     ", found {0}"#)]
    FileIdentifierError(String),

    #[error(r#"File version too low: Expected "> 4.1", found {0}"#)]
    FileVersioningError(String),

    #[error("Invalid block identifier: Expected {expected:?}, got {actual:?}")]
    BlockIDError {
        actual: String,
        expected: String,
    },

    #[error("I/O error: {0}")]
    IOError(#[from] std::io::Error),

    #[error("Invalid version string: {0}")]
    InvalidVersionString(String),

    #[error("Block linking error: {0}")]
    BlockLinkError(String),

    #[error("Block serialization error: {0}")]
    BlockSerializationError(String),

    #[error("Conversion chain too deep: maximum depth of {max_depth} exceeded")]
    ConversionChainTooDeep { max_depth: usize },

    #[error("Conversion chain cycle detected at block address {address:#x}")]
    ConversionChainCycle { address: u64 },
}

fn channel_not_found_message(name: &str, group: &Option<String>) -> String {
    match group {
        Some(g) => format!("Channel '{}' not found in group '{}'", name, g),
        None => format!("Channel '{}' not found", name),
    }
}

impl MdfError {
    /// Build a [`MdfError::ChannelNotFound`].
    pub fn channel_not_found(name: &str, group: Option<&str>) -> Self {
        MdfError::ChannelNotFound { name: name.to_string(), group: group.map(str::to_string) }
    }
}
