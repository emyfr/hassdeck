use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Deserialize)]
pub struct Config {
    #[serde(default = "default_brightness")]
    pub brightness: u8,
    pub home_assistant: HomeAssistantConfig,
    #[serde(default)]
    pub keys: Vec<KeyConfig>,
    #[serde(default)]
    pub web: WebConfig,
}

fn default_brightness() -> u8 {
    80
}

#[derive(Debug, Deserialize)]
pub struct WebConfig {
    #[serde(default = "default_web_bind")]
    pub bind: String,
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            bind: default_web_bind(),
        }
    }
}

fn default_web_bind() -> String {
    "0.0.0.0:8080".to_string()
}

#[derive(Debug, Deserialize)]
pub struct HomeAssistantConfig {
    pub url: String,
    pub token: String,
}

#[derive(Debug, Deserialize)]
pub struct KeyConfig {
    pub key: u8,
    #[serde(default)]
    pub icon: Option<String>,
    pub action: Action,
}

/// Tipo di azione eseguita alla pressione di un tasto. Home Assistant è il
/// primo tipo implementato; altri tipi (webhook, comando shell locale) si
/// aggiungono come nuove varianti, vedi ANALYSIS.md.
#[derive(Debug, Deserialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    HomeAssistant { service: String, entity_id: String },
}

pub fn load(path: &Path) -> anyhow::Result<Config> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("impossibile leggere {}: {}", path.display(), e))?;
    let config: Config = toml::from_str(&text)
        .map_err(|e| anyhow::anyhow!("configurazione non valida in {}: {}", path.display(), e))?;
    Ok(config)
}
