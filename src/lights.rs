//! Livello di luminosita' mostrato sui tasti luce (azione `light`): lo sfondo
//! del tasto e' riempito dal basso in proporzione alla luminosita' attuale,
//! sotto l'icona. Lo stato delle luci e' letto da Home Assistant ogni
//! `POLL_INTERVAL`, cosi' il tasto segue anche le modifiche fatte altrove.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

use crate::actions::{self, EntityState};
use crate::config::Action;
use crate::hold;
use crate::icons;
use crate::system;
use crate::web::AppState;

const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Tasti luce di cui si sta regolando la luminosita' con una pressione
/// prolungata: la lettura periodica li salta, per non mostrare uno stato di
/// Home Assistant non ancora aggiornato.
pub type AdjustingLights = Arc<Mutex<HashSet<u8>>>;

/// Livello 0-100 da mostrare: la luminosita' se la luce e' accesa (100 per
/// le luci solo on/off), 0 se e' spenta o non disponibile.
pub fn level_from_state(light: &EntityState) -> u32 {
    if light.state != "on" {
        return 0;
    }
    light.brightness.map_or(100, hold::brightness_pct)
}

/// Mostra il livello sul tasto e lo salva nella cache delle icone, cosi'
/// anche pressione e rilascio mostrano il riempimento invece dell'icona sola.
pub async fn show_level(state: &AppState, key: u8, level_pct: u32) {
    let key_icons = {
        let mut cache = state.icon_cache.write().await;
        let overlay = cache.get(&key).map(|icons| icons.overlay.clone());
        let key_icons = icons::level_icons(overlay.as_ref(), level_pct);
        cache.insert(key, key_icons.clone());
        key_icons
    };
    if let Err(e) = icons::write_icon_to_device(&state.device, key, key_icons.normal).await {
        eprintln!("Luci: errore scrivendo il tasto {key}: {e}");
    }
}

/// Aggiorna ogni `POLL_INTERVAL` i tasti luce, riscrivendo sul deck solo
/// quelli il cui livello e' cambiato. `light_wake` forza una nuova lettura e
/// la riscrittura di tutti i tasti (dopo una pressione o un cambio di icona).
pub async fn run(state: AppState) {
    // Livello mostrato su ciascun tasto, per evitare riscritture inutili.
    let mut shown: HashMap<u8, u32> = HashMap::new();

    loop {
        let (keys, ha) = {
            let config = state.config.read().await;
            let keys: Vec<(u8, String)> = config
                .keys
                .iter()
                .filter_map(|k| match &k.action {
                    Some(Action::Light { entity_id, .. }) => Some((k.key, entity_id.clone())),
                    _ => None,
                })
                .collect();
            (keys, config.home_assistant.clone())
        };

        let removed: Vec<u8> = shown.keys().copied().filter(|k| !keys.iter().any(|(key, _)| key == k)).collect();
        for key in removed {
            shown.remove(&key);
            system::restore_key(&state, key).await;
        }

        for (key, entity_id) in keys {
            if state.adjusting_lights.lock().await.contains(&key) {
                continue;
            }
            // Home Assistant non raggiungibile: il tasto resta com'e'.
            let Ok(light) = actions::get_state(&ha, &entity_id).await else {
                continue;
            };
            let level = level_from_state(&light);
            if shown.get(&key) == Some(&level) {
                continue;
            }
            show_level(&state, key, level).await;
            shown.insert(key, level);
        }

        tokio::select! {
            _ = tokio::time::sleep(POLL_INTERVAL) => {}
            _ = state.light_wake.notified() => shown.clear(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn light(state: &str, brightness: Option<u8>) -> EntityState {
        EntityState { state: state.to_string(), unit: None, brightness }
    }

    #[test]
    fn level_follows_brightness_when_on() {
        assert_eq!(level_from_state(&light("on", Some(255))), 100);
        assert_eq!(level_from_state(&light("on", Some(128))), 50);
        assert_eq!(level_from_state(&light("on", None)), 100);
    }

    #[test]
    fn level_is_zero_when_off_or_unavailable() {
        assert_eq!(level_from_state(&light("off", None)), 0);
        assert_eq!(level_from_state(&light("unavailable", None)), 0);
    }
}
