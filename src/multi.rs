//! Pulsanti multipli (`[[multi]]` in config): controlli che usano piu' tasti
//! insieme. Per ora il termostato: un tasto alza e uno abbassa la temperatura
//! impostata di un'entita' `climate`, ed entrambi la mostrano con una freccia,
//! riletta da Home Assistant ogni `POLL_INTERVAL`.

use ab_glyph::FontRef;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

use crate::actions::{self, ClimateState};
use crate::config::{self, HomeAssistantConfig, MultiButton};
use crate::hold::{self, HoldTracker};
use crate::icons;
use crate::system;
use crate::web::{self, AppState, KeyEvent};

const POLL_INTERVAL: Duration = Duration::from_secs(2);
/// Passo usato se ne' la configurazione ne' l'entita' ne indicano uno.
const DEFAULT_STEP: f64 = 0.5;
/// Attesa dopo l'ultima variazione prima di tornare a leggere la temperatura
/// da Home Assistant, che per un attimo puo' riportare ancora quella vecchia.
const SETTLE_DELAY: Duration = Duration::from_millis(1500);

const UP_ARROW: &str = "▲";
const DOWN_ARROW: &str = "▼";

/// Termostati di cui si sta cambiando la temperatura: la lettura periodica li
/// salta, per non mostrare un valore di Home Assistant non ancora aggiornato.
pub type AdjustingClimates = Arc<Mutex<HashSet<String>>>;

fn font() -> FontRef<'static> {
    FontRef::try_from_slice(system::FONT_DATA).expect("font DejaVu incluso non valido")
}

/// Passo della regolazione: quello configurato, altrimenti quello dichiarato
/// dall'entita' (`target_temp_step`), altrimenti `DEFAULT_STEP`.
fn effective_step(configured: Option<f64>, climate: Option<&ClimateState>) -> f64 {
    configured
        .or(climate.and_then(|c| c.step))
        .filter(|s| s.is_finite() && *s > 0.0)
        .unwrap_or(DEFAULT_STEP)
}

/// Temperatura successiva: un passo sopra o sotto `current`, allineata ai
/// multipli del passo e limitata a `min`/`max` dell'entita'.
fn next_target(current: f64, up: bool, step: f64, min: Option<f64>, max: Option<f64>) -> f64 {
    let raw = if up { current + step } else { current - step };
    let mut next = (raw / step).round() * step;
    if let Some(min) = min {
        next = next.max(min);
    }
    if let Some(max) = max {
        next = next.min(max);
    }
    (next * 100.0).round() / 100.0
}

/// Temperatura come mostrata sul tasto: decimali solo se il passo li usa.
fn format_temperature(value: Option<f64>, step: f64) -> String {
    match value {
        Some(v) => {
            let decimals = if step.fract() == 0.0 { 0 } else { 1 };
            format!("{v:.decimals$}°").replace('.', ",")
        }
        None => "—".to_string(),
    }
}

/// Modalita' del termostato, mostrata in basso sul tasto.
fn mode_label(state: &str) -> String {
    match state {
        "heat" => "CALDO".to_string(),
        "cool" => "FREDDO".to_string(),
        "heat_cool" | "auto" => "AUTO".to_string(),
        "dry" => "DEUMID.".to_string(),
        "fan_only" => "VENTIL.".to_string(),
        "off" => "SPENTO".to_string(),
        "" | "unavailable" | "unknown" => "N/D".to_string(),
        other => other.to_uppercase(),
    }
}

/// Disegna sui due tasti del termostato la freccia, `value` e `mode`, e li
/// salva nella cache delle icone, cosi' anche la pressione mostra il valore.
async fn show_climate(state: &AppState, font: &FontRef<'_>, up_key: u8, down_key: u8, value: &str, mode: &str) {
    for (key, arrow) in [(up_key, UP_ARROW), (down_key, DOWN_ARROW)] {
        let key_icons = icons::prepare_key_icons(system::render_metric(font, arrow, value, mode));
        if let Err(e) = icons::write_icon_to_device(&state.device, key, key_icons.normal.clone()).await {
            eprintln!("Termostato: errore scrivendo il tasto {key}: {e}");
        }
        state.icon_cache.write().await.insert(key, key_icons);
        send_event(state, key, arrow, value, mode);
    }
}

/// Valore per la pagina di monitoraggio, mostrato come un dato di sistema.
fn send_event(state: &AppState, key: u8, arrow: &str, value: &str, mode: &str) {
    let _ = state.events.send(KeyEvent::System {
        physical_key: key,
        label: arrow.to_string(),
        value: value.to_string(),
        unit: mode.to_string(),
    });
}

/// Aggiorna ogni `POLL_INTERVAL` i tasti dei pulsanti multipli, riscrivendo
/// sul deck solo quelli il cui contenuto e' cambiato. `multi_wake` forza una
/// nuova lettura e la riscrittura di tutti i tasti (dopo una modifica della
/// configurazione o un cambio di icona).
pub async fn run(state: AppState) {
    let font = font();
    // Contenuto mostrato su ciascun tasto, per evitare riscritture inutili.
    let mut shown: HashMap<u8, String> = HashMap::new();

    loop {
        let (groups, ha) = {
            let config = state.config.read().await;
            (config.multi.clone(), config.home_assistant.clone())
        };

        let keys: Vec<u8> = groups.iter().flat_map(MultiButton::keys).collect();
        let removed: Vec<u8> = shown.keys().copied().filter(|k| !keys.contains(k)).collect();
        for key in removed {
            shown.remove(&key);
            system::restore_key(&state, key).await;
        }

        for group in groups {
            let MultiButton::Climate { entity_id, up_key, down_key, step } = group;
            if state.adjusting_climates.lock().await.contains(&entity_id) {
                continue;
            }
            let climate = actions::get_climate_state(&ha, &entity_id).await.ok();
            let step = effective_step(step, climate.as_ref());
            let value = format_temperature(climate.as_ref().and_then(|c| c.target), step);
            let mode = mode_label(climate.as_ref().map_or("", |c| c.state.as_str()));

            let signature = format!("{entity_id}:{value}:{mode}");
            if shown.get(&up_key) == Some(&signature) && shown.get(&down_key) == Some(&signature) {
                send_event(&state, up_key, UP_ARROW, &value, &mode);
                send_event(&state, down_key, DOWN_ARROW, &value, &mode);
                continue;
            }
            show_climate(&state, &font, up_key, down_key, &value, &mode).await;
            shown.insert(up_key, signature.clone());
            shown.insert(down_key, signature);
        }

        tokio::select! {
            _ = tokio::time::sleep(POLL_INTERVAL) => {}
            _ = state.multi_wake.notified() => shown.clear(),
        }
    }
}

/// Cambia la temperatura del termostato di cui fa parte `physical_key`: un
/// passo subito e, se il tasto resta premuto oltre `hold::LIGHT_LONG_PRESS`,
/// un passo ogni `hold::LIGHT_STEP_INTERVAL` fino al rilascio o al limite
/// dell'entita'. Con una pressione `id` non registrata in `holds` (tasto
/// attivato da web) fa un passo solo.
pub async fn run_climate_hold(state: &AppState, physical_key: u8, holds: &HoldTracker, id: u64) {
    let (group, ha) = {
        let config = state.config.read().await;
        (config::multi_for_key(&config, physical_key).cloned(), config.home_assistant.clone())
    };
    let Some(MultiButton::Climate { entity_id, up_key, down_key, step }) = group else {
        return;
    };

    state.adjusting_climates.lock().await.insert(entity_id.clone());
    let keys = (up_key, down_key);
    adjust_climate(state, physical_key, holds, id, &ha, &entity_id, keys, step).await;
    tokio::time::sleep(SETTLE_DELAY).await;
    state.adjusting_climates.lock().await.remove(&entity_id);
    // Riallinea i tasti alla temperatura letta da Home Assistant.
    state.multi_wake.notify_one();
}

#[allow(clippy::too_many_arguments)]
async fn adjust_climate(
    state: &AppState,
    physical_key: u8,
    holds: &HoldTracker,
    id: u64,
    ha: &HomeAssistantConfig,
    entity_id: &str,
    (up_key, down_key): (u8, u8),
    configured_step: Option<f64>,
) {
    let climate = match actions::get_climate_state(ha, entity_id).await {
        Ok(climate) => climate,
        Err(e) => {
            web::record_action_result(state, physical_key, Err(e)).await;
            return;
        }
    };
    let Some(mut target) = climate.target else {
        let message = format!("{entity_id} non ha una temperatura impostata (spento o regolato per intervallo?)");
        web::record_action_result(state, physical_key, Err(anyhow::anyhow!(message))).await;
        return;
    };
    let up = physical_key == up_key;
    let step = effective_step(configured_step, Some(&climate));
    let mode = mode_label(&climate.state);
    let font = font();

    for n in 0.. {
        if n > 0 {
            let pause = if n == 1 { hold::LIGHT_LONG_PRESS } else { hold::LIGHT_STEP_INTERVAL };
            tokio::time::sleep(pause).await;
            if !holds.is_held(physical_key, id).await {
                break;
            }
        }
        let next = next_target(target, up, step, climate.min, climate.max);
        if next == target {
            break; // al limite dell'entita'
        }
        let result = actions::set_climate_temperature(ha, entity_id, next).await;
        let success = result.is_ok();
        web::record_action_result(state, physical_key, result).await;
        if !success {
            break;
        }
        target = next;
        println!("Tasto {physical_key}: {entity_id} impostato a {target}");
        let value = format_temperature(Some(target), step);
        show_climate(state, &font, up_key, down_key, &value, &mode).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn climate(step: Option<f64>) -> ClimateState {
        ClimateState { state: "heat".to_string(), target: Some(20.0), step, min: Some(7.0), max: Some(35.0) }
    }

    #[test]
    fn step_prefers_config_then_entity() {
        assert_eq!(effective_step(Some(1.0), Some(&climate(Some(0.1)))), 1.0);
        assert_eq!(effective_step(None, Some(&climate(Some(0.1)))), 0.1);
        assert_eq!(effective_step(None, Some(&climate(None))), DEFAULT_STEP);
        assert_eq!(effective_step(Some(0.0), None), DEFAULT_STEP);
    }

    #[test]
    fn next_target_moves_one_step_and_snaps() {
        assert_eq!(next_target(20.0, true, 0.5, None, None), 20.5);
        assert_eq!(next_target(20.0, false, 0.5, None, None), 19.5);
        assert_eq!(next_target(20.3, true, 0.5, None, None), 21.0);
        assert_eq!(next_target(21.7, true, 0.1, None, None), 21.8);
    }

    #[test]
    fn next_target_stays_within_limits() {
        assert_eq!(next_target(35.0, true, 0.5, Some(7.0), Some(35.0)), 35.0);
        assert_eq!(next_target(7.0, false, 0.5, Some(7.0), Some(35.0)), 7.0);
    }

    #[test]
    fn temperature_uses_decimals_only_when_needed() {
        assert_eq!(format_temperature(Some(21.5), 0.5), "21,5°");
        assert_eq!(format_temperature(Some(21.0), 1.0), "21°");
        assert_eq!(format_temperature(None, 0.5), "—");
    }

    #[test]
    fn font_has_the_arrows() {
        use ab_glyph::Font;
        let font = font();
        for arrow in [UP_ARROW, DOWN_ARROW] {
            let c = arrow.chars().next().unwrap();
            assert_ne!(font.glyph_id(c).0, 0, "glifo mancante per {arrow}");
        }
    }
}
