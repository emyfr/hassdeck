//! Barra verticale usata come indicatore di livello: legge un sensore di
//! Home Assistant e riempie le tre zone dal basso in proporzione al valore
//! compreso tra `min` e `max` (sezione `[bar]` di config.toml).
//!
//! Le tre zone sono schermi 95×95 separati (indici di scrittura 15, 16, 17,
//! dall'alto in basso): il riempimento viene calcolato su un'unica barra
//! virtuale alta 285 pixel e poi diviso tra le zone, cosi' sale in modo
//! continuo da una zona all'altra. Vedi ANALYSIS.md.

use image::{DynamicImage, Rgb, RgbImage};
use serde::Serialize;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

use crate::actions;
use crate::icons;
use crate::keymap::BAR_WRITE_INDICES;
use crate::web::{AppState, KeyEvent};

const SEGMENT_SIZE: u32 = 95;
const SEGMENT_COUNT: usize = 3;
const BAR_HEIGHT: u32 = SEGMENT_SIZE * SEGMENT_COUNT as u32;
const POLL_INTERVAL: Duration = Duration::from_secs(2);
/// Giri consecutivi con scritture fallite dopo cui il deck e' considerato
/// perso e il servizio esce, per farsi riavviare da systemd.
const MAX_WRITE_FAILURES: u32 = 3;

const EMPTY_COLOR: [u8; 3] = [0, 0, 0];
/// Colore uniforme mostrato quando il sensore non e' leggibile, per non
/// confonderlo con un valore pari al minimo (barra vuota).
const UNAVAILABLE_COLOR: [u8; 3] = [60, 60, 60];
/// Gradiente dal basso verso l'alto: verde, giallo, rosso.
const GRADIENT: [[u8; 3]; 3] = [[0, 200, 70], [255, 210, 0], [230, 30, 30]];

#[derive(Debug, Clone, Default, Serialize)]
pub struct BarStatus {
    pub entity_id: Option<String>,
    pub value: Option<f64>,
    pub unit: Option<String>,
    /// Frazione riempita (0-1), `None` se il sensore non e' disponibile o la
    /// barra non e' configurata.
    pub level: Option<f64>,
    pub error: Option<String>,
}

pub type SharedBarStatus = Arc<RwLock<BarStatus>>;

/// Contenuto di una zona: righe accese dal basso, oppure sensore non
/// disponibile. Serve a riscrivere sul deck solo le zone che cambiano.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Segment {
    Lit(u32),
    Unavailable,
}

/// Frazione della barra da riempire per `value`, limitata a 0-1.
pub fn level(value: f64, min: f64, max: f64) -> f64 {
    ((value - min) / (max - min)).clamp(0.0, 1.0)
}

/// Righe accese in ciascuna zona (dall'alto) per `filled` righe accese
/// sull'intera barra.
fn segments_for(filled: u32) -> [Segment; SEGMENT_COUNT] {
    std::array::from_fn(|segment| {
        let from_bottom = (SEGMENT_COUNT - 1 - segment) as u32 * SEGMENT_SIZE;
        Segment::Lit(filled.saturating_sub(from_bottom).min(SEGMENT_SIZE))
    })
}

fn gradient_color(t: f64) -> [u8; 3] {
    let t = t.clamp(0.0, 1.0) * (GRADIENT.len() - 1) as f64;
    let i = (t.floor() as usize).min(GRADIENT.len() - 2);
    let f = t - i as f64;
    std::array::from_fn(|c| {
        (GRADIENT[i][c] as f64 * (1.0 - f) + GRADIENT[i + 1][c] as f64 * f).round() as u8
    })
}

fn render_segment(segment: usize, content: Segment) -> DynamicImage {
    let mut img = RgbImage::from_pixel(SEGMENT_SIZE, SEGMENT_SIZE, Rgb(EMPTY_COLOR));
    match content {
        Segment::Unavailable => {
            img = RgbImage::from_pixel(SEGMENT_SIZE, SEGMENT_SIZE, Rgb(UNAVAILABLE_COLOR));
        }
        Segment::Lit(rows) => {
            for y in SEGMENT_SIZE - rows..SEGMENT_SIZE {
                // Colore in base all'altezza sull'intera barra, non nella zona.
                let bar_y = segment as u32 * SEGMENT_SIZE + y;
                let color = gradient_color((BAR_HEIGHT - 1 - bar_y) as f64 / (BAR_HEIGHT - 1) as f64);
                for x in 0..SEGMENT_SIZE {
                    img.put_pixel(x, y, Rgb(color));
                }
            }
        }
    }
    DynamicImage::ImageRgb8(img)
}

/// Legge il sensore configurato e calcola cosa mostrare.
async fn read_bar(state: &AppState) -> (BarStatus, [Segment; SEGMENT_COUNT]) {
    let (bar, ha) = {
        let config = state.config.read().await;
        (config.bar.clone(), config.home_assistant.clone())
    };
    let Some(bar) = bar else {
        return (BarStatus::default(), segments_for(0));
    };

    let mut status = BarStatus {
        entity_id: Some(bar.entity_id.clone()),
        ..Default::default()
    };
    let unavailable = [Segment::Unavailable; SEGMENT_COUNT];

    if bar.max <= bar.min {
        status.error = Some("max deve essere maggiore di min".to_string());
        return (status, unavailable);
    }

    match actions::get_state(&ha, &bar.entity_id).await {
        Ok(entity) => {
            status.unit = entity.unit;
            match entity.state.parse::<f64>() {
                Ok(value) if value.is_finite() => {
                    let level = level(value, bar.min, bar.max);
                    status.value = Some(value);
                    status.level = Some(level);
                    let filled = (level * BAR_HEIGHT as f64).round() as u32;
                    (status, segments_for(filled))
                }
                _ => {
                    status.error = Some(format!("stato del sensore: '{}'", entity.state));
                    (status, unavailable)
                }
            }
        }
        Err(e) => {
            status.error = Some(e.to_string());
            (status, unavailable)
        }
    }
}

/// Aggiorna la barra ogni `POLL_INTERVAL`, o subito quando la sua
/// configurazione cambia (`AppState::bar_wake`).
pub async fn run(state: AppState) {
    let mut shown: Option<[Segment; SEGMENT_COUNT]> = None;
    let mut last_error: Option<String> = None;
    let mut write_failures = 0;

    loop {
        let (status, segments) = read_bar(&state).await;

        if status.error != last_error {
            if let (Some(entity), Some(error)) = (&status.entity_id, &status.error) {
                eprintln!("Barra: impossibile leggere {entity}: {error}");
            }
            last_error = status.error.clone();
        }

        let mut write_failed = false;
        for (segment, content) in segments.iter().enumerate() {
            if shown.is_some_and(|s| s[segment] == *content) {
                continue;
            }
            let image = render_segment(segment, *content);
            if let Err(e) = icons::write_image_to_index(&state.device, BAR_WRITE_INDICES[segment], image).await {
                eprintln!("Barra: errore scrivendo la zona {}: {e}", segment + 1);
                write_failed = true;
            }
        }
        // Dopo un errore riscrive tutte le zone al giro successivo.
        shown = if write_failed { None } else { Some(segments) };
        write_failures = if write_failed { write_failures + 1 } else { 0 };
        if write_failures >= MAX_WRITE_FAILURES {
            eprintln!("Barra: il deck non accetta piu' scritture, esco per riconnettermi");
            std::process::exit(1);
        }

        *state.bar_status.write().await = status.clone();
        let _ = state.events.send(KeyEvent::Bar(status));

        tokio::select! {
            _ = tokio::time::sleep(POLL_INTERVAL) => {}
            _ = state.bar_wake.notified() => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_is_clamped() {
        assert_eq!(level(1250.0, 0.0, 2500.0), 0.5);
        assert_eq!(level(-10.0, 0.0, 2500.0), 0.0);
        assert_eq!(level(4000.0, 0.0, 2500.0), 1.0);
    }

    #[test]
    fn segments_fill_from_bottom() {
        use Segment::Lit;
        assert_eq!(segments_for(0), [Lit(0), Lit(0), Lit(0)]);
        assert_eq!(segments_for(50), [Lit(0), Lit(0), Lit(50)]);
        assert_eq!(segments_for(95), [Lit(0), Lit(0), Lit(95)]);
        assert_eq!(segments_for(150), [Lit(0), Lit(55), Lit(95)]);
        assert_eq!(segments_for(BAR_HEIGHT), [Lit(95), Lit(95), Lit(95)]);
    }

    #[test]
    fn gradient_goes_green_yellow_red() {
        assert_eq!(gradient_color(0.0), GRADIENT[0]);
        assert_eq!(gradient_color(0.5), GRADIENT[1]);
        assert_eq!(gradient_color(1.0), GRADIENT[2]);
    }
}
