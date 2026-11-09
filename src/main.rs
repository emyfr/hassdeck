mod actions;
mod auth;
mod bar;
mod config;
mod hold;
mod icons;
mod keymap;
mod lights;
mod system;
mod web;

use image::open as open_image;
use mirajazz::{
    device::{list_devices, Device, DeviceQuery},
    error::MirajazzError,
    types::DeviceInput,
};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use tokio::sync::{broadcast, mpsc, Notify, RwLock};
use web::{AppState, KeyEvent};

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
        ha_client: reqwest::Client::new(),
        auth: auth::Auth::default(),
        bar_status: Arc::new(RwLock::new(bar::BarStatus::default())),
        bar_wake: Arc::new(Notify::new()),
        system_wake: Arc::new(Notify::new()),
        light_wake: Arc::new(Notify::new()),
        adjusting_lights: Default::default(),
    };

    let web_addr = {
        let config = config.read().await;
        config
            .web
            .bind
            .parse()
            .map_err(|e| anyhow::anyhow!("indirizzo web '{}' non valido: {e}", config.web.bind))?
    };
    tokio::spawn(web::serve(app_state.clone(), web_addr));

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
                let key_icons = icons::prepare_key_icons(image);
                icons::write_icon_to_device(&device, key_config.key, key_icons.normal.clone()).await?;
                icon_cache.write().await.insert(key_config.key, key_icons);
                println!("Icona caricata per tasto {}", key_config.key);
            }
        }
        device.flush().await?;
    }

    tokio::spawn(bar::run(app_state.clone()));
    tokio::spawn(system::run(app_state.clone()));
    tokio::spawn(lights::run(app_state.clone()));

    let (tx, mut rx) = mpsc::unbounded_channel();
    KEY_EVENTS.set(tx).ok();

    let reader = device.get_reader(on_key_event);
    let reader_task = tokio::spawn(async move {
        loop {
            if let Err(e) = reader.read(None).await {
                // Deck scollegato o controller USB caduto: esce con errore,
                // cosi' systemd riavvia il servizio e si ricollega al deck.
                eprintln!("Lettura dal deck fallita, esco per riconnettermi: {e}");
                std::process::exit(1);
            }
        }
    });

    println!("In ascolto. Premi un tasto sul deck per eseguire l'azione configurata.");

    let holds = hold::HoldTracker::default();
    let light_holds = hold::HoldTracker::default();
    let light_directions = hold::LightDirections::default();

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
            web::show_key_icon(&app_state, physical_key, true).await;

            // Un tasto luce usa gia' la pressione prolungata per la
            // luminosita', quindi la protezione non si applica.
            let (light, protected) = {
                let config = config.read().await;
                let key_config = config.keys.iter().find(|k| k.key == physical_key);
                let light = key_config
                    .is_some_and(|k| matches!(k.action, Some(config::Action::Light { .. })));
                (light, !light && key_config.is_some_and(|k| k.protected))
            };
            if light {
                // Se il tasto e' ancora premuto dopo LIGHT_LONG_PRESS la
                // luminosita' cambia a passi fino al rilascio; altrimenti il
                // rilascio esegue l'azione normale (accende o spegne).
                let id = light_holds.press(physical_key).await;
                let (light_holds, light_directions, app_state) =
                    (light_holds.clone(), light_directions.clone(), app_state.clone());
                tokio::spawn(async move {
                    tokio::time::sleep(hold::LIGHT_LONG_PRESS).await;
                    if light_holds.fire_if_held(physical_key, id).await {
                        web::run_light_hold(&app_state, physical_key, &light_holds, id, &light_directions)
                            .await;
                    }
                });
            } else if protected {
                // L'azione parte solo se il tasto e' ancora premuto allo
                // scadere del tempo, e l'icona torna normale per segnalare
                // che si puo' rilasciare.
                let id = holds.press(physical_key).await;
                let (holds, app_state) = (holds.clone(), app_state.clone());
                tokio::spawn(async move {
                    tokio::time::sleep(hold::HOLD_DURATION).await;
                    if holds.fire_if_held(physical_key, id).await {
                        web::show_key_icon(&app_state, physical_key, false).await;
                        web::run_key_action(&app_state, physical_key).await;
                    }
                });
            }
            // Gli altri tasti eseguono l'azione solo al rilascio, per evitare
            // di eseguirla due volte (una per la pressione, una per il rilascio).
            continue;
        }

        match holds.release(physical_key).await {
            // Tasto protetto tenuto abbastanza: l'azione e' gia' partita.
            Some(true) => continue,
            Some(false) => {
                web::show_key_icon(&app_state, physical_key, false).await;
                println!("Tasto {physical_key} protetto rilasciato troppo presto: nessuna azione");
                let _ = events_tx.send(KeyEvent::HoldRequired { physical_key });
                continue;
            }
            None => {}
        }

        // Tasto luce tenuto premuto: la luminosita' e' gia' stata regolata.
        // Rilasciato prima, prosegue come un tasto normale (accende o spegne).
        if light_holds.release(physical_key).await == Some(true) {
            web::show_key_icon(&app_state, physical_key, false).await;
            continue;
        }

        // Ripristina l'icona a riposo, se il tasto ne ha una.
        web::show_key_icon(&app_state, physical_key, false).await;

        // Stessa funzione usata quando il tasto viene attivato da web, cosi'
        // il comportamento (esecuzione azione, stato, evento) e' identico.
        web::run_key_action(&app_state, physical_key).await;
    }

    reader_task.abort();
    device.shutdown().await?;

    Ok(())
}
