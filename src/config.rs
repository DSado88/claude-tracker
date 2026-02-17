use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::error::ConfigError;

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum AuthMethod {
    #[default]
    SessionKey,
    #[serde(alias = "o_auth")]
    #[serde(rename = "oauth")]
    OAuth,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub settings: Settings,
    #[serde(default)]
    pub accounts: Vec<AccountConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default = "default_poll_interval")]
    pub poll_interval_secs: u64,
    #[serde(default)]
    pub active_account: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountConfig {
    pub name: String,
    #[serde(default)]
    pub org_id: String,
    #[serde(default)]
    pub auth_method: AuthMethod,
}

fn default_poll_interval() -> u64 {
    180
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            poll_interval_secs: default_poll_interval(),
            active_account: 0,
        }
    }
}

pub fn config_dir() -> Result<PathBuf, ConfigError> {
    let home = dirs::home_dir().ok_or(ConfigError::NoHomeDir)?;
    Ok(home.join(".config").join("claude-tracker"))
}

pub fn config_path() -> Result<PathBuf, ConfigError> {
    Ok(config_dir()?.join("config.toml"))
}

pub fn load_or_init() -> Result<Config, ConfigError> {
    let path = config_path()?;
    if path.exists() {
        let contents = std::fs::read_to_string(&path)?;
        let config: Config = toml::from_str(&contents)?;
        Ok(config)
    } else {
        let config = Config {
            settings: Settings::default(),
            accounts: vec![],
        };
        save(&config)?;
        Ok(config)
    }
}

pub fn save(config: &Config) -> Result<(), ConfigError> {
    let path = config_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let toml_str = toml::to_string_pretty(config)?;
    std::fs::write(&path, toml_str)?;
    Ok(())
}
