use crate::keymap;
use image::DynamicImage;
use mirajazz::{
    device::Device,
    types::{ImageFormat, ImageMirroring, ImageMode, ImageRotation},
};

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
