use thiserror::Error;

#[derive(Error, Debug)]
pub enum TrackerError {
    #[error("Config error: {0}")]
    Config(#[from] ConfigError),

    #[error("API error for account '{account}': {message}")]
    Api { account: String, message: String },

    #[error("Swap error: {0}")]
    Swap(String),

    #[error("Keyring error: {0}")]
    Keyring(String),
}

#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("Could not determine home directory")]
    NoHomeDir,

    #[error("Failed to read config: {0}")]
    ReadFailed(#[from] std::io::Error),

    #[error("Failed to parse config: {0}")]
    ParseFailed(#[from] toml::de::Error),

    #[error("Failed to serialize config: {0}")]
    SerializeFailed(#[from] toml::ser::Error),

    #[error("Validation error: {0}")]
    Validation(String),
}
