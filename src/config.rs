use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Config {
    #[serde(default = "default_brightness")]
    pub brightness: u8,
    #[serde(default)]
    pub home_assistant: HomeAssistantConfig,
    #[serde(default)]
    pub keys: Vec<KeyConfig>,
    #[serde(default)]
    pub web: WebConfig,
}

fn default_brightness() -> u8 {
    80
}

#[derive(Debug, Deserialize, Serialize, Clone)]
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

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct HomeAssistantConfig {
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub token: String,
    /// Verifica il certificato TLS quando `url` e' https. Disattivabile per
    /// istanze locali con certificato self-signed.
    #[serde(default = "default_verify_tls")]
    pub verify_tls: bool,
}

impl Default for HomeAssistantConfig {
    fn default() -> Self {
        Self {
            url: String::new(),
            token: String::new(),
            verify_tls: default_verify_tls(),
        }
    }
}

fn default_verify_tls() -> bool {
    true
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct KeyConfig {
    pub key: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<Action>,
}

/// Tipo di azione eseguita alla pressione di un tasto. Home Assistant è il
/// primo tipo implementato; altri tipi (webhook, comando shell locale) si
/// aggiungono come nuove varianti, vedi ANALYSIS.md.
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    HomeAssistant { service: String, entity_id: String },
    Url {
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        method: Option<String>,
    },
}

pub fn load(path: &Path) -> anyhow::Result<Config> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("impossibile leggere {}: {}", path.display(), e))?;
    let config: Config = toml::from_str(&text)
        .map_err(|e| anyhow::anyhow!("configurazione non valida in {}: {}", path.display(), e))?;
    Ok(config)
}

pub fn save(config: &Config, path: &Path) -> anyhow::Result<()> {
    let text = toml::to_string_pretty(config)
        .map_err(|e| anyhow::anyhow!("impossibile serializzare la configurazione: {e}"))?;
    std::fs::write(path, text)
        .map_err(|e| anyhow::anyhow!("impossibile scrivere {}: {e}", path.display()))?;
    Ok(())
}

/// Inserisce o aggiorna l'icona di un tasto, creando la voce se non esiste.
pub fn set_key_icon(config: &mut Config, key: u8, icon: String) {
    if let Some(kc) = config.keys.iter_mut().find(|kc| kc.key == key) {
        kc.icon = Some(icon);
    } else {
        config.keys.push(KeyConfig {
            key,
            icon: Some(icon),
            action: None,
        });
    }
}

/// Inserisce o aggiorna l'azione di un tasto, creando la voce se non esiste.
pub fn set_key_action(config: &mut Config, key: u8, action: Action) {
    if let Some(kc) = config.keys.iter_mut().find(|kc| kc.key == key) {
        kc.action = Some(action);
    } else {
        config.keys.push(KeyConfig {
            key,
            icon: None,
            action: Some(action),
        });
    }
}

/// Rimuove l'azione di un tasto, lasciando intatta l'eventuale icona.
pub fn clear_key_action(config: &mut Config, key: u8) {
    if let Some(kc) = config.keys.iter_mut().find(|kc| kc.key == key) {
        kc.action = None;
    }
}
