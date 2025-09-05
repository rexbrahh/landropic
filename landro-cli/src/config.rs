use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub device_name: String,
    pub device_id: String,
    pub daemon_port: u16,
    pub storage_path: PathBuf,
}

impl Config {
    /// Save configuration to the default location
    pub fn save(&self) -> Result<()> {
        let config_path = get_config_path()?;
        
        // Ensure directory exists
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create config directory: {}", parent.display()))?;
        }

        let content = toml::to_string_pretty(self)
            .context("Failed to serialize config to TOML")?;

        std::fs::write(&config_path, content)
            .with_context(|| format!("Failed to write config file: {}", config_path.display()))?;

        Ok(())
    }

    /// Load configuration from the default location
    pub fn load() -> Result<Config> {
        let config_path = get_config_path()?;
        
        let content = std::fs::read_to_string(&config_path)
            .with_context(|| format!("Failed to read config file: {}", config_path.display()))?;

        let config: Config = toml::from_str(&content)
            .with_context(|| format!("Failed to parse config file: {}", config_path.display()))?;

        Ok(config)
    }

    /// Check if configuration file exists
    pub fn exists() -> Result<bool> {
        let config_path = get_config_path()?;
        Ok(config_path.exists())
    }
}

/// Get the path to the config file
pub fn get_config_path() -> Result<PathBuf> {
    let home = dirs::home_dir()
        .context("Home directory not found")?;
    Ok(home.join(".landropic").join("config.toml"))
}

/// Get the default storage path
pub fn default_storage_path() -> Result<PathBuf> {
    let home = dirs::home_dir()
        .context("Home directory not found")?;
    Ok(home.join(".landropic").join("storage"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use std::env;

    #[test]
    fn test_config_serialization() {
        let config = Config {
            device_name: "Test Device".to_string(),
            device_id: "abcd1234".to_string(),
            daemon_port: 7890,
            storage_path: PathBuf::from("/tmp/test"),
        };

        let toml_str = toml::to_string_pretty(&config).unwrap();
        assert!(toml_str.contains("device_name"));
        assert!(toml_str.contains("Test Device"));
        assert!(toml_str.contains("daemon_port = 7890"));

        let deserialized: Config = toml::from_str(&toml_str).unwrap();
        assert_eq!(deserialized.device_name, config.device_name);
        assert_eq!(deserialized.device_id, config.device_id);
        assert_eq!(deserialized.daemon_port, config.daemon_port);
        assert_eq!(deserialized.storage_path, config.storage_path);
    }

    #[test]
    fn test_save_and_load_config() {
        let temp_dir = tempdir().unwrap();
        let old_home = env::var("HOME").ok();
        
        // Set temporary HOME directory
        env::set_var("HOME", temp_dir.path());

        let config = Config {
            device_name: "Test Device".to_string(),
            device_id: "abcd1234".to_string(),
            daemon_port: 7890,
            storage_path: temp_dir.path().join("storage"),
        };

        // Save config
        config.save().unwrap();

        // Load config
        let loaded = Config::load().unwrap();
        assert_eq!(loaded.device_name, config.device_name);
        assert_eq!(loaded.device_id, config.device_id);
        assert_eq!(loaded.daemon_port, config.daemon_port);
        assert_eq!(loaded.storage_path, config.storage_path);

        // Restore original HOME
        if let Some(home) = old_home {
            env::set_var("HOME", home);
        } else {
            env::remove_var("HOME");
        }
    }
}