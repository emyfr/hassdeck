mod actions;
mod config;
mod keymap;
mod web;

use image::open as open_image;
use mirajazz::{
    device::{list_devices, Device, DeviceQuery},
    error::MirajazzError,
    types::{DeviceInput, ImageFormat, ImageMirroring, ImageMode, ImageRotation},
};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use tokio::sync::{mpsc, RwLock};
use web::{ActionResultInfo, KeyPressInfo, SharedStatus};

const QUERY: DeviceQuery = DeviceQuery::new(65440, 1, 0x1500, 0x3003);
const KEY_COUNT: usize = 18;
const ENCODER_COUNT: usize = 0;
const PROTOCOL_VERSION: usize = 3;

fn image_format_for_write_index(index: u8) -> ImageFormat {
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

static KEY_EVENTS: OnceLock<mpsc::UnboundedSender<(u8, u8)>> = OnceLock::new();

fn on_key_event(key: u8, state: u8) -> Result<DeviceInput, MirajazzError> {
    if let Some(tx) = KEY_EVENTS.get() {
        let _ = tx.send((key, state));
    }
    Ok(DeviceInput::NoData)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config_path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("config.toml"));

    let config = config::load(&config_path)?;
    println!("Configurazione caricata da {}", config_path.display());

    let devices = list_devices(&[QUERY]).await?;
    let dev = devices
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("nessun deck Soomfon trovato (VID 0x1500, PID 0x3003)"))?;

    let device = Device::connect(&dev, PROTOCOL_VERSION, KEY_COUNT, ENCODER_COUNT).await?;
    println!("Connesso al deck, serial {}", device.serial_number());

    let status: SharedStatus = Arc::new(RwLock::new(web::Status {
        connected: true,
        serial: device.serial_number().to_string(),
        ..Default::default()
    }));

    let web_addr = config
        .web
        .bind
        .parse()
        .map_err(|e| anyhow::anyhow!("indirizzo web '{}' non valido: {e}", config.web.bind))?;
    tokio::spawn(web::serve(status.clone(), web_addr));

    // set_brightness attiva l'handshake di inizializzazione del dispositivo:
    // senza, il deck non riporta le pressioni dei tasti (vedi ANALYSIS.md).
    device.set_brightness(config.brightness).await?;
    device.clear_all_button_images().await?;

    for key_config in &config.keys {
        if let Some(icon_path) = &key_config.icon {
            let write_index = keymap::write_index_for_physical_key(key_config.key);
            let format = image_format_for_write_index(write_index);
            let image = open_image(icon_path)
                .map_err(|e| anyhow::anyhow!("impossibile caricare icona '{icon_path}': {e}"))?;
            device.set_button_image(write_index, format, image).await?;
            println!(
                "Icona caricata per tasto {} (indice {write_index})",
                key_config.key
            );
        }
    }
    device.flush().await?;

    let (tx, mut rx) = mpsc::unbounded_channel();
    KEY_EVENTS.set(tx).ok();

    let reader = device.get_reader(on_key_event);
    let reader_task = tokio::spawn(async move {
        loop {
            if reader.read(None).await.is_err() {
                break;
            }
        }
    });

    let ha_client = reqwest::Client::new();

    println!("In ascolto. Premi un tasto sul deck per eseguire l'azione configurata.");

    while let Some((read_index, state)) = rx.recv().await {
        // Reagisce solo al rilascio, per evitare di eseguire l'azione due volte
        // (una per la pressione, una per il rilascio).
        if state != 0 {
            continue;
        }

        let Some(physical_key) = keymap::physical_key_for_read_index(read_index) else {
            continue; // barra verticale o indice non riconosciuto
        };

        status.write().await.last_key_press = Some(KeyPressInfo {
            physical_key,
            at_unix: web::now_unix(),
        });

        let Some(key_config) = config.keys.iter().find(|k| k.key == physical_key) else {
            println!("Tasto {physical_key} premuto, nessuna azione configurata");
            continue;
        };

        println!("Tasto {physical_key} premuto: eseguo azione");
        let result = actions::execute(&ha_client, &config.home_assistant, &key_config.action).await;

        let (success, message) = match &result {
            Ok(()) => (true, "OK".to_string()),
            Err(e) => (false, e.to_string()),
        };
        status.write().await.last_action_result = Some(ActionResultInfo {
            physical_key,
            success,
            message,
            at_unix: web::now_unix(),
        });

        if let Err(e) = result {
            eprintln!("Errore eseguendo l'azione per il tasto {physical_key}: {e}");
        }
    }

    reader_task.abort();
    device.shutdown().await?;

    Ok(())
}
