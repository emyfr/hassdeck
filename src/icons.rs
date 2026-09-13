use crate::keymap;
use image::{imageops::FilterType, DynamicImage};
use mirajazz::{
    device::Device,
    types::{ImageFormat, ImageMirroring, ImageMode, ImageRotation},
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Colore e intensita' della tinta applicata all'icona quando il tasto e'
/// premuto — un blend di colore e' molto piu' visibile di un semplice
/// scurimento su uno schermo LCD piccolo e a basso contrasto.
const PRESSED_TINT_COLOR: [u8; 3] = [255, 140, 0];
const PRESSED_TINT_ALPHA: f32 = 0.65;

pub fn image_format_for_write_index(index: u8) -> ImageFormat {
    // Indice 17 = ultimo slot della barra verticale, dimensione diversa nel
    // modello ereditato da AKP153 — non ancora confermata con un'icona reale.
    let size = if index == 17 { (82, 82) } else { (95, 95) };
    ImageFormat {
        mode: ImageMode::JPEG,
        size,
        rotation: ImageRotation::Rot90,
        mirror: ImageMirroring::Both,
    }
}

fn tint(image: &DynamicImage, color: [u8; 3], alpha: f32) -> DynamicImage {
    let mut rgb = image.to_rgb8();
    for pixel in rgb.pixels_mut() {
        for i in 0..3 {
            let blended = pixel.0[i] as f32 * (1.0 - alpha) + color[i] as f32 * alpha;
            pixel.0[i] = blended.round().clamp(0.0, 255.0) as u8;
        }
    }
    DynamicImage::ImageRgb8(rgb)
}

/// Le due varianti pronte da inviare al deck per un tasto: quella a riposo e
/// quella (scurita) mostrata mentre il tasto e' premuto. Gia' ridimensionate
/// alla dimensione richiesta dal canale vendor, cosi' non serve rielaborare
/// l'immagine ad ogni pressione — solo ruotare/specchiare/codificare in JPEG,
/// gia' fatto internamente da `mirajazz` ad ogni scrittura.
#[derive(Clone)]
pub struct KeyIcons {
    pub normal: DynamicImage,
    pub pressed: DynamicImage,
}

pub type IconCache = Arc<RwLock<HashMap<u8, KeyIcons>>>;

pub fn new_icon_cache() -> IconCache {
    Arc::new(RwLock::new(HashMap::new()))
}

/// Ridimensiona l'icona caricata dall'utente alla dimensione corretta per il
/// tasto e prepara subito anche la variante "premuta".
pub fn prepare_key_icons(physical_key: u8, image: DynamicImage) -> KeyIcons {
    let write_index = keymap::write_index_for_physical_key(physical_key);
    let (w, h) = image_format_for_write_index(write_index).size;
    let normal = image.resize_exact(w as u32, h as u32, FilterType::Lanczos3);
    let pressed = tint(&normal, PRESSED_TINT_COLOR, PRESSED_TINT_ALPHA);
    KeyIcons { normal, pressed }
}

pub async fn write_icon_to_device(
    device: &Device,
    physical_key: u8,
    image: DynamicImage,
) -> anyhow::Result<()> {
    let write_index = keymap::write_index_for_physical_key(physical_key);
    let format = image_format_for_write_index(write_index);
    device.set_button_image(write_index, format, image).await?;
    device.flush().await?;
    Ok(())
}
