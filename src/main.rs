mod actions;
mod config;
mod icons;
mod keymap;
mod web;

use image::open as open_image;
use mirajazz::{
    device::{list_devices, Device, DeviceQuery},
    error::MirajazzError,
    types::DeviceInput,
};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use tokio::sync::{broadcast, mpsc, RwLock};
use web::{ActionResultInfo, AppState, KeyEvent, KeyPressInfo};

const QUERY: DeviceQuery = DeviceQuery::new(65440, 1, 0x1500, 0x3003);
const KEY_COUNT: usize = 18;
const ENCODER_COUNT: usize = 0;
const PROTOCOL_VERSION: usize = 3;
const ICONS_DIR: &str = "icons";

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

    let device = Arc::new(Device::connect(&dev, PROTOCOL_VERSION, KEY_COUNT, ENCODER_COUNT).await?);
    println!("Connesso al deck, serial {}", device.serial_number());

    let status = Arc::new(RwLock::new(web::Status {
        connected: true,
        serial: device.serial_number().to_string(),
        ..Default::default()
    }));
    let (events_tx, _) = broadcast::channel::<KeyEvent>(64);
    let config = Arc::new(RwLock::new(config));

    let icon_cache = icons::new_icon_cache();

    let app_state = AppState {
        status: status.clone(),
        events: events_tx.clone(),
        device: device.clone(),
        config: config.clone(),
        config_path: Arc::new(config_path),
        icons_dir: Arc::new(PathBuf::from(ICONS_DIR)),
        icon_cache: icon_cache.clone(),
    };

    let web_addr = {
        let config = config.read().await;
        config
            .web
            .bind
            .parse()
            .map_err(|e| anyhow::anyhow!("indirizzo web '{}' non valido: {e}", config.web.bind))?
    };
    tokio::spawn(web::serve(app_state, web_addr));

    // set_brightness attiva l'handshake di inizializzazione del dispositivo:
    // senza, il deck non riporta le pressioni dei tasti (vedi ANALYSIS.md).
    {
        let config = config.read().await;
        device.set_brightness(config.brightness).await?;
        device.clear_all_button_images().await?;

        for key_config in &config.keys {
            if let Some(icon_path) = &key_config.icon {
                let image = open_image(icon_path)
                    .map_err(|e| anyhow::anyhow!("impossibile caricare icona '{icon_path}': {e}"))?;
                let key_icons = icons::prepare_key_icons(key_config.key, image);
                icons::write_icon_to_device(&device, key_config.key, key_icons.normal.clone()).await?;
                icon_cache.write().await.insert(key_config.key, key_icons);
                println!("Icona caricata per tasto {}", key_config.key);
            }
        }
        device.flush().await?;
    }

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
        let Some(physical_key) = keymap::physical_key_for_read_index(read_index) else {
            continue; // barra verticale o indice non riconosciuto
        };

        let pressed = state != 0;
        let _ = events_tx.send(KeyEvent::Key {
            physical_key,
            pressed,
        });

        if pressed {
            // Mostra subito la variante scurita, se il tasto ha un'icona.
            if let Some(icons) = icon_cache.read().await.get(&physical_key) {
                let _ = icons::write_icon_to_device(&device, physical_key, icons.pressed.clone()).await;
            }
            // Esegue l'azione solo al rilascio, per evitare di eseguirla due
            // volte (una per la pressione, una per il rilascio).
            continue;
        }

        // Ripristina l'icona a riposo, se il tasto ne ha una.
        if let Some(icons) = icon_cache.read().await.get(&physical_key) {
            let _ = icons::write_icon_to_device(&device, physical_key, icons.normal.clone()).await;
        }

        status.write().await.last_key_press = Some(KeyPressInfo {
            physical_key,
            at_unix: web::now_unix(),
        });

        let config = config.read().await;
        let Some(action) = config
            .keys
            .iter()
            .find(|k| k.key == physical_key)
            .and_then(|k| k.action.as_ref())
        else {
            println!("Tasto {physical_key} premuto, nessuna azione configurata");
            continue;
        };

        println!("Tasto {physical_key} premuto: eseguo azione");
        let result = actions::execute(&ha_client, &config.home_assistant, action).await;
        drop(config);

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
        let _ = events_tx.send(KeyEvent::ActionResult {
            physical_key,
            success,
        });

        if let Err(e) = result {
            eprintln!("Errore eseguendo l'azione per il tasto {physical_key}: {e}");
        }
    }

    reader_task.abort();
    device.shutdown().await?;

    Ok(())
}
