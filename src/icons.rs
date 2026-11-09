use crate::keymap;
use image::{imageops::FilterType, DynamicImage, Rgba, RgbaImage};
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

/// Sfondo e riempimento del tasto mentre si regola la luminosita' di una luce.
const LEVEL_BACKGROUND: [u8; 3] = [18, 18, 22];
const LEVEL_FILL: [u8; 3] = [255, 176, 0];

/// Formato immagine del deck: tutti gli schermi, tasti e zone della barra,
/// sono 95×95 (verificato con immagini di prova, vedi ANALYSIS.md).
pub fn image_format() -> ImageFormat {
    ImageFormat {
        mode: ImageMode::JPEG,
        size: (95, 95),
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
    /// Icona ridimensionata con la trasparenza originale, disegnata sopra il
    /// riempimento di `level_image`.
    pub overlay: RgbaImage,
}

pub type IconCache = Arc<RwLock<HashMap<u8, KeyIcons>>>;

pub fn new_icon_cache() -> IconCache {
    Arc::new(RwLock::new(HashMap::new()))
}

/// Ridimensiona l'icona caricata dall'utente alla dimensione corretta per il
/// tasto e prepara subito anche la variante "premuta".
pub fn prepare_key_icons(image: DynamicImage) -> KeyIcons {
    let (w, h) = image_format().size;
    let overlay = image
        .resize_exact(w as u32, h as u32, FilterType::Lanczos3)
        .to_rgba8();
    // Il deck riceve JPEG, senza trasparenza: le parti trasparenti
    // dell'icona diventano nere invece di mostrare il colore nascosto.
    let mut flattened = RgbaImage::from_pixel(w as u32, h as u32, Rgba([0, 0, 0, 255]));
    image::imageops::overlay(&mut flattened, &overlay, 0, 0);
    let normal = DynamicImage::ImageRgb8(DynamicImage::ImageRgba8(flattened).to_rgb8());
    let pressed = tint(&normal, PRESSED_TINT_COLOR, PRESSED_TINT_ALPHA);
    KeyIcons { normal, pressed, overlay }
}

/// Tasto riempito dal basso per `level_pct` (0-100) della sua altezza, con
/// sopra l'icona del tasto se ne ha una: mostrato mentre si regola la
/// luminosita' di una luce.
pub fn level_image(icons: Option<&KeyIcons>, level_pct: u32) -> DynamicImage {
    let (w, h) = image_format().size;
    let (w, h) = (w as u32, h as u32);
    let fill_from = h - h * level_pct.min(100) / 100;
    let mut image = RgbaImage::from_fn(w, h, |_, y| {
        let [r, g, b] = if y >= fill_from { LEVEL_FILL } else { LEVEL_BACKGROUND };
        Rgba([r, g, b, 255])
    });
    if let Some(icons) = icons {
        image::imageops::overlay(&mut image, &icons.overlay, 0, 0);
    }
    DynamicImage::ImageRgba8(image)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_fills_from_the_bottom() {
        let image = level_image(None, 50).to_rgba8();
        let (_, h) = image.dimensions();
        assert_eq!(image.get_pixel(0, 0).0[..3], LEVEL_BACKGROUND);
        assert_eq!(image.get_pixel(0, h - 1).0[..3], LEVEL_FILL);
        assert_eq!(image.get_pixel(0, h / 2 - 1).0[..3], LEVEL_BACKGROUND);
        assert_eq!(image.get_pixel(0, h / 2 + 1).0[..3], LEVEL_FILL);
    }

    #[test]
    fn level_zero_and_full() {
        let empty = level_image(None, 0).to_rgba8();
        let full = level_image(None, 100).to_rgba8();
        let (_, h) = empty.dimensions();
        assert_eq!(empty.get_pixel(0, h - 1).0[..3], LEVEL_BACKGROUND);
        assert_eq!(full.get_pixel(0, 0).0[..3], LEVEL_FILL);
    }
}

pub async fn write_icon_to_device(
    device: &Device,
    physical_key: u8,
    image: DynamicImage,
) -> anyhow::Result<()> {
    let write_index = keymap::write_index_for_physical_key(physical_key);
    write_image_to_index(device, write_index, image).await
}

/// Scrive un'immagine su uno schermo dato il suo indice logico di scrittura
/// (0-14 tasti, 15-17 barra verticale).
pub async fn write_image_to_index(
    device: &Device,
    write_index: u8,
    image: DynamicImage,
) -> anyhow::Result<()> {
    device.set_button_image(write_index, image_format(), image).await?;
    device.flush().await?;
    Ok(())
}
