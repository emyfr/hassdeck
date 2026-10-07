//! Dati di sistema mostrati sui tasti (azione `system`): al posto dell'icona
//! il tasto mostra un parametro del Raspberry (CPU, temperatura, tensione,
//! memoria), letto e ridisegnato ogni `POLL_INTERVAL`.
//!
//! Il testo e' disegnato con il font DejaVu Sans Bold incluso nel binario
//! (`assets/fonts`, licenza in `LICENSE-DejaVu.txt`).

use ab_glyph::{point, Font, FontRef, PxScale, ScaleFont};
use image::{open as open_image, DynamicImage, Rgb, RgbImage};
use serde::Serialize;
use std::collections::HashMap;
use std::time::Duration;

use crate::config::Action;
use crate::icons;
use crate::web::{AppState, KeyEvent};

const POLL_INTERVAL: Duration = Duration::from_secs(2);
const FONT_DATA: &[u8] = include_bytes!("../assets/fonts/DejaVuSans-Bold.ttf");

const SIZE: u32 = 95;
const BACKGROUND: [u8; 3] = [18, 18, 22];
const LABEL_COLOR: [u8; 3] = [160, 160, 170];
const VALUE_COLOR: [u8; 3] = [255, 255, 255];

/// Parametro di sistema selezionabile nell'editor. L'elenco viene servito
/// da `GET /api/system/metrics`, quindi aggiungerne uno qui basta a
/// renderlo disponibile anche nell'interfaccia web.
#[derive(Debug, Serialize)]
pub struct MetricInfo {
    pub id: &'static str,
    pub name: &'static str,
    /// Etichetta breve mostrata sul tasto.
    pub label: &'static str,
    pub unit: &'static str,
    /// Cifre decimali del valore mostrato.
    #[serde(skip)]
    decimals: usize,
}

pub const METRICS: &[MetricInfo] = &[
    MetricInfo { id: "cpu_percent", name: "Utilizzo CPU", label: "CPU", unit: "%", decimals: 0 },
    MetricInfo { id: "cpu_temp", name: "Temperatura CPU", label: "TEMP", unit: "°C", decimals: 1 },
    MetricInfo { id: "supply_voltage", name: "Tensione di alimentazione (EXT5V)", label: "ALIM", unit: "V", decimals: 2 },
    MetricInfo { id: "memory_percent", name: "Memoria RAM utilizzata", label: "RAM", unit: "%", decimals: 0 },
];

pub fn metric_info(id: &str) -> Option<&'static MetricInfo> {
    METRICS.iter().find(|m| m.id == id)
}

/// Utilizzo CPU calcolato dalla differenza tra due letture di /proc/stat.
#[derive(Default)]
struct CpuSampler {
    previous: Option<(u64, u64)>,
}

impl CpuSampler {
    fn sample(&mut self) -> Option<f64> {
        let stat = std::fs::read_to_string("/proc/stat").ok()?;
        let fields: Vec<u64> = stat
            .lines()
            .next()?
            .split_whitespace()
            .skip(1)
            .take(8)
            .filter_map(|v| v.parse().ok())
            .collect();
        if fields.len() < 5 {
            return None;
        }
        // idle + iowait
        let idle = fields[3] + fields[4];
        let total: u64 = fields.iter().sum();
        let previous = self.previous.replace((idle, total));
        let (prev_idle, prev_total) = previous?;
        let total_delta = total.saturating_sub(prev_total);
        if total_delta == 0 {
            return None;
        }
        let idle_delta = idle.saturating_sub(prev_idle);
        Some(100.0 * (1.0 - idle_delta as f64 / total_delta as f64))
    }
}

fn cpu_temp() -> Option<f64> {
    let raw = std::fs::read_to_string("/sys/class/thermal/thermal_zone0/temp").ok()?;
    Some(raw.trim().parse::<f64>().ok()? / 1000.0)
}

/// Tensione di ingresso letta dal PMIC del Raspberry Pi 5. Richiede che
/// l'utente del servizio sia nel gruppo `video` (accesso a /dev/vcio_gencmd).
async fn supply_voltage() -> Option<f64> {
    let output = tokio::process::Command::new("vcgencmd")
        .args(["pmic_read_adc", "EXT5V_V"])
        .output()
        .await
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    text.trim().rsplit('=').next()?.trim_end_matches('V').parse().ok()
}

fn memory_percent() -> Option<f64> {
    let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
    let field = |name: &str| -> Option<f64> {
        meminfo
            .lines()
            .find(|l| l.starts_with(name))?
            .split_whitespace()
            .nth(1)?
            .parse()
            .ok()
    };
    let total = field("MemTotal:")?;
    let available = field("MemAvailable:")?;
    Some(100.0 * (total - available) / total)
}

async fn read_metric(id: &str, cpu: Option<f64>) -> Option<f64> {
    match id {
        "cpu_percent" => cpu,
        "cpu_temp" => cpu_temp(),
        "supply_voltage" => supply_voltage().await,
        "memory_percent" => memory_percent(),
        _ => None,
    }
}

fn text_width(font: &FontRef, scale: PxScale, text: &str) -> f32 {
    let scaled = font.as_scaled(scale);
    let mut width = 0.0;
    let mut previous = None;
    for c in text.chars() {
        let id = scaled.glyph_id(c);
        if let Some(prev) = previous {
            width += scaled.kern(prev, id);
        }
        width += scaled.h_advance(id);
        previous = Some(id);
    }
    width
}

/// Disegna `text` centrato orizzontalmente con la linea di base a `baseline`,
/// riducendo la dimensione finche' non sta in `max_width`.
fn draw_centered(img: &mut RgbImage, font: &FontRef, text: &str, size: f32, baseline: f32, max_width: f32, color: [u8; 3]) {
    let mut scale = PxScale::from(size);
    let width = text_width(font, scale, text);
    if width > max_width {
        scale = PxScale::from(size * max_width / width);
    }
    let scaled = font.as_scaled(scale);
    let mut x = (SIZE as f32 - text_width(font, scale, text)) / 2.0;
    let mut previous = None;

    for c in text.chars() {
        let id = scaled.glyph_id(c);
        if let Some(prev) = previous {
            x += scaled.kern(prev, id);
        }
        let glyph = id.with_scale_and_position(scale, point(x, baseline));
        x += scaled.h_advance(id);
        previous = Some(id);

        let Some(outline) = font.outline_glyph(glyph) else {
            continue;
        };
        let bounds = outline.px_bounds();
        outline.draw(|gx, gy, coverage| {
            let px = bounds.min.x as i32 + gx as i32;
            let py = bounds.min.y as i32 + gy as i32;
            if px < 0 || py < 0 || px >= SIZE as i32 || py >= SIZE as i32 {
                return;
            }
            let pixel = img.get_pixel_mut(px as u32, py as u32);
            for i in 0..3 {
                let blended = pixel.0[i] as f32 * (1.0 - coverage) + color[i] as f32 * coverage;
                pixel.0[i] = blended.round() as u8;
            }
        });
    }
}

fn render_metric(font: &FontRef, label: &str, value: &str, unit: &str) -> DynamicImage {
    let mut img = RgbImage::from_pixel(SIZE, SIZE, Rgb(BACKGROUND));
    let max_width = SIZE as f32 - 8.0;
    draw_centered(&mut img, font, label, 17.0, 22.0, max_width, LABEL_COLOR);
    draw_centered(&mut img, font, value, 34.0, 62.0, max_width, VALUE_COLOR);
    draw_centered(&mut img, font, unit, 17.0, 86.0, max_width, LABEL_COLOR);
    DynamicImage::ImageRgb8(img)
}

fn format_value(info: &MetricInfo, value: Option<f64>) -> String {
    match value {
        Some(v) => format!("{v:.*}", info.decimals).replace('.', ","),
        None => "—".to_string(),
    }
}

/// Riporta il tasto alla sua icona configurata (o lo spegne) quando smette
/// di mostrare un dato di sistema.
async fn restore_key(state: &AppState, key: u8) {
    let icon = {
        let config = state.config.read().await;
        config.keys.iter().find(|k| k.key == key).and_then(|k| k.icon.clone())
    };
    let image = icon.and_then(|path| open_image(path).ok());
    match image {
        Some(image) => {
            let key_icons = icons::prepare_key_icons(image);
            let _ = icons::write_icon_to_device(&state.device, key, key_icons.normal.clone()).await;
            state.icon_cache.write().await.insert(key, key_icons);
        }
        None => {
            let blank = DynamicImage::ImageRgb8(RgbImage::new(SIZE, SIZE));
            let _ = icons::write_icon_to_device(&state.device, key, blank).await;
            state.icon_cache.write().await.remove(&key);
        }
    }
}

/// Aggiorna ogni `POLL_INTERVAL` i tasti configurati con un dato di sistema,
/// riscrivendo sul deck solo quelli il cui testo e' cambiato.
pub async fn run(state: AppState) {
    let font = FontRef::try_from_slice(FONT_DATA).expect("font DejaVu incluso non valido");
    let mut cpu = CpuSampler::default();
    // Testo mostrato su ciascun tasto, per evitare riscritture inutili.
    let mut shown: HashMap<u8, String> = HashMap::new();

    loop {
        let keys: Vec<(u8, String)> = {
            let config = state.config.read().await;
            config
                .keys
                .iter()
                .filter_map(|k| match &k.action {
                    Some(Action::System { metric }) => Some((k.key, metric.clone())),
                    _ => None,
                })
                .collect()
        };

        let removed: Vec<u8> = shown.keys().copied().filter(|k| !keys.iter().any(|(key, _)| key == k)).collect();
        for key in removed {
            shown.remove(&key);
            restore_key(&state, key).await;
        }

        // Campionata a ogni giro, anche senza tasti, per avere sempre un delta.
        let cpu_percent = cpu.sample();

        for (key, metric) in keys {
            let (label, unit, value) = match metric_info(&metric) {
                Some(info) => (info.label, info.unit, format_value(info, read_metric(info.id, cpu_percent).await)),
                None => ("?", "", "—".to_string()),
            };

            let _ = state.events.send(KeyEvent::System {
                physical_key: key,
                label: label.to_string(),
                value: value.clone(),
                unit: unit.to_string(),
            });

            let signature = format!("{metric}:{value}");
            if shown.get(&key) == Some(&signature) {
                continue;
            }
            let key_icons = icons::prepare_key_icons(render_metric(&font, label, &value, unit));
            match icons::write_icon_to_device(&state.device, key, key_icons.normal.clone()).await {
                Ok(()) => {
                    shown.insert(key, signature);
                }
                Err(e) => eprintln!("Dati di sistema: errore scrivendo il tasto {key}: {e}"),
            }
            // Anche la variante premuta mostra il valore, cosi' la pressione
            // non fa ricomparire l'icona caricata.
            state.icon_cache.write().await.insert(key, key_icons);
        }

        tokio::select! {
            _ = tokio::time::sleep(POLL_INTERVAL) => {}
            _ = state.system_wake.notified() => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metric_ids_are_unique() {
        for (i, a) in METRICS.iter().enumerate() {
            assert!(METRICS[i + 1..].iter().all(|b| b.id != a.id), "id duplicato: {}", a.id);
        }
    }

    #[test]
    fn values_use_metric_decimals_and_comma() {
        let temp = metric_info("cpu_temp").unwrap();
        assert_eq!(format_value(temp, Some(48.53)), "48,5");
        let cpu = metric_info("cpu_percent").unwrap();
        assert_eq!(format_value(cpu, Some(12.6)), "13");
        assert_eq!(format_value(cpu, None), "—");
    }

    #[test]
    fn font_renders_all_metric_texts() {
        let font = FontRef::try_from_slice(FONT_DATA).unwrap();
        for m in METRICS {
            let img = render_metric(&font, m.label, "100", m.unit).to_rgb8();
            assert!(img.pixels().any(|p| p.0 == VALUE_COLOR), "nessun pixel del valore per {}", m.id);
        }
    }
}
