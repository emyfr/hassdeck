use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Multipart, Path as AxumPath, State,
    },
    http::StatusCode,
    response::{Html, IntoResponse},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{broadcast, Notify, RwLock};

use crate::actions;
use crate::auth::{self, Auth};
use crate::bar::{BarStatus, SharedBarStatus};
use crate::config::{self, Action, BarConfig, Config, HomeAssistantConfig, MultiButton};
use crate::hold::{self, HoldTracker, LightDirections};
use crate::icon_library::{self, LibraryIcon};
use crate::icons::{self, IconCache};
use crate::lights::{self, AdjustingLights};
use crate::multi::{self, AdjustingClimates};
use crate::system;
use mirajazz::device::Device;

#[derive(Debug, Clone, Serialize)]
pub struct KeyPressInfo {
    pub physical_key: u8,
    pub at_unix: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ActionResultInfo {
    pub physical_key: u8,
    pub success: bool,
    pub message: String,
    pub at_unix: u64,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct Status {
    pub connected: bool,
    pub serial: String,
    pub last_key_press: Option<KeyPressInfo>,
    pub last_action_result: Option<ActionResultInfo>,
}

pub type SharedStatus = Arc<RwLock<Status>>;
pub type SharedConfig = Arc<RwLock<Config>>;

/// Evento in tempo reale inoltrato ai client connessi via WebSocket, usato
/// per animare la griglia dei tasti e la barra nella pagina di monitoraggio.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum KeyEvent {
    Key { physical_key: u8, pressed: bool },
    ActionResult { physical_key: u8, success: bool },
    /// Tasto protetto rilasciato prima di `hold::HOLD_DURATION`: nessuna azione.
    HoldRequired { physical_key: u8 },
    Bar(BarStatus),
    System {
        physical_key: u8,
        label: String,
        value: String,
        unit: String,
    },
}

#[derive(Clone)]
pub struct AppState {
    pub status: SharedStatus,
    pub events: broadcast::Sender<KeyEvent>,
    pub device: Arc<Device>,
    pub config: SharedConfig,
    pub config_path: Arc<PathBuf>,
    pub icons_dir: Arc<PathBuf>,
    pub icon_cache: IconCache,
    pub ha_client: reqwest::Client,
    pub auth: Auth,
    pub bar_status: SharedBarStatus,
    /// Sveglia il task della barra quando la sua configurazione cambia.
    pub bar_wake: Arc<Notify>,
    /// Sveglia il task dei dati di sistema quando cambia l'azione di un tasto.
    pub system_wake: Arc<Notify>,
    /// Sveglia il task dei tasti luce per rileggere subito lo stato delle luci.
    pub light_wake: Arc<Notify>,
    pub adjusting_lights: AdjustingLights,
    /// Sveglia il task dei pulsanti multipli per ridisegnarne subito i tasti.
    pub multi_wake: Arc<Notify>,
    pub adjusting_climates: AdjustingClimates,
}

/// Durata (ms) del lampeggio "premuto" mostrato quando il tasto viene
/// attivato da web, dato che non esiste una vera pressione da tenere.
const WEB_PRESS_FLASH_MS: u64 = 180;

/// Mostra la variante "premuta" dell'icona del tasto (se presente) per un
/// breve istante, poi ripristina quella normale — usato quando il tasto
/// viene attivato dall'interfaccia web anziche' da una pressione fisica.
async fn flash_key_icon(state: &AppState, physical_key: u8) {
    let icons = state.icon_cache.read().await.get(&physical_key).cloned();

    let _ = state.events.send(KeyEvent::Key {
        physical_key,
        pressed: true,
    });
    if let Some(icons) = &icons {
        let _ = icons::write_icon_to_device(&state.device, physical_key, icons.pressed.clone()).await;
    }

    tokio::time::sleep(Duration::from_millis(WEB_PRESS_FLASH_MS)).await;

    let _ = state.events.send(KeyEvent::Key {
        physical_key,
        pressed: false,
    });
    if let Some(icons) = &icons {
        let _ = icons::write_icon_to_device(&state.device, physical_key, icons.normal.clone()).await;
    }
}

/// Mostra la variante premuta o a riposo dell'icona del tasto, se ne ha una.
pub async fn show_key_icon(state: &AppState, physical_key: u8, pressed: bool) {
    if let Some(icons) = state.icon_cache.read().await.get(&physical_key) {
        let image = if pressed { icons.pressed.clone() } else { icons.normal.clone() };
        let _ = icons::write_icon_to_device(&state.device, physical_key, image).await;
    }
}

/// Esegue l'azione configurata per un tasto e registra l'esito, sia che
/// l'attivazione provenga da una pressione fisica sia da un click web.
pub async fn run_key_action(state: &AppState, physical_key: u8) {
    state.status.write().await.last_key_press = Some(KeyPressInfo {
        physical_key,
        at_unix: now_unix(),
    });

    let (action, ha, in_multi) = {
        let config = state.config.read().await;
        let action = config
            .keys
            .iter()
            .find(|k| k.key == physical_key)
            .and_then(|k| k.action.clone());
        let in_multi = config::multi_for_key(&config, physical_key).is_some();
        (action, config.home_assistant.clone(), in_multi)
    };

    // Tasto di un pulsante multiplo attivato da web: un passo solo, dato che
    // nessuna pressione resta in corso.
    if in_multi {
        multi::run_climate_hold(state, physical_key, &HoldTracker::default(), 0).await;
        return;
    }

    let Some(action) = action else {
        println!("Tasto {physical_key} premuto, nessuna azione configurata");
        return;
    };

    println!("Tasto {physical_key} premuto: eseguo azione");
    let result = actions::execute(&state.ha_client, &ha, &action).await;
    record_action_result(state, physical_key, result).await;

    // La luce e' stata accesa o spenta: il tasto mostra subito il nuovo livello.
    if matches!(action, Action::Light { .. }) {
        state.light_wake.notify_one();
    }
}

/// Regola la luminosita' della luce associata al tasto finche' la pressione
/// `id` resta in corso: un passo ogni `hold::LIGHT_STEP_INTERVAL`, salendo o
/// scendendo secondo `directions`, e si ferma arrivati al 100% o allo 0%, al
/// rilascio o al primo errore di Home Assistant. Intanto il tasto si riempie
/// dal basso in proporzione alla luminosita' (vedi `lights.rs`).
pub async fn run_light_hold(
    state: &AppState,
    physical_key: u8,
    holds: &HoldTracker,
    id: u64,
    directions: &LightDirections,
) {
    let (action, ha) = {
        let config = state.config.read().await;
        let action = config
            .keys
            .iter()
            .find(|k| k.key == physical_key)
            .and_then(|k| k.action.clone());
        (action, config.home_assistant.clone())
    };

    let Some(Action::Light { entity_id, step_pct }) = action else {
        return;
    };

    state.adjusting_lights.lock().await.insert(physical_key);
    adjust_light(state, physical_key, holds, id, directions, &ha, &entity_id, step_pct).await;
    state.adjusting_lights.lock().await.remove(&physical_key);
    // Riallinea il tasto allo stato letto da Home Assistant.
    state.light_wake.notify_one();
}

#[allow(clippy::too_many_arguments)]
async fn adjust_light(
    state: &AppState,
    physical_key: u8,
    holds: &HoldTracker,
    id: u64,
    directions: &LightDirections,
    ha: &HomeAssistantConfig,
    entity_id: &str,
    step_pct: u8,
) {

    let brightness = match actions::get_state(ha, entity_id).await {
        Ok(light) if light.state == "on" => light.brightness,
        Ok(_) => None,
        Err(e) => {
            record_action_result(state, physical_key, Err(e)).await;
            return;
        }
    };
    let up = directions.next(physical_key, brightness).await;
    let steps = hold::light_steps(brightness, up, step_pct);
    let step = if up { step_pct as i8 } else { -(step_pct as i8) };
    // Livello stimato dai passi inviati, senza rileggere lo stato ogni volta.
    let mut level = brightness.map_or(0, hold::brightness_pct);
    lights::show_level(state, physical_key, level).await;
    println!(
        "Tasto {physical_key} tenuto premuto: luminosita' {} fino a {steps} passi",
        if up { "in salita" } else { "in discesa" }
    );

    for n in 0..steps {
        if n > 0 {
            tokio::time::sleep(hold::LIGHT_STEP_INTERVAL).await;
            if !holds.is_held(physical_key, id).await {
                break;
            }
        }
        let result = actions::light_brightness_step(ha, entity_id, step).await;
        let success = result.is_ok();
        record_action_result(state, physical_key, result).await;
        if !success {
            break;
        }
        level = if up {
            (level + u32::from(step_pct)).min(100)
        } else {
            level.saturating_sub(u32::from(step_pct))
        };
        lights::show_level(state, physical_key, level).await;
    }
}

/// Registra l'esito di un'azione nello stato e lo inoltra ai client web.
pub(crate) async fn record_action_result(state: &AppState, physical_key: u8, result: anyhow::Result<()>) {
    let (success, message) = match &result {
        Ok(()) => (true, "OK".to_string()),
        Err(e) => (false, e.to_string()),
    };
    state.status.write().await.last_action_result = Some(ActionResultInfo {
        physical_key,
        success,
        message,
        at_unix: now_unix(),
    });
    let _ = state.events.send(KeyEvent::ActionResult {
        physical_key,
        success,
    });

    if let Err(e) = result {
        eprintln!("Errore eseguendo l'azione per il tasto {physical_key}: {e}");
    }
}

pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn internal_err<E: std::fmt::Display>(e: E) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

fn bad_request(msg: impl Into<String>) -> (StatusCode, String) {
    (StatusCode::BAD_REQUEST, msg.into())
}

fn check_key_range(key: u8) -> Result<(), (StatusCode, String)> {
    if (1..=15).contains(&key) {
        Ok(())
    } else {
        Err(bad_request("tasto non valido: deve essere tra 1 e 15"))
    }
}

async fn status_handler(State(state): State<AppState>) -> Json<Status> {
    Json(state.status.read().await.clone())
}

async fn index_handler() -> Html<&'static str> {
    Html(include_str!("../web/index.html"))
}

async fn configure_handler() -> Html<&'static str> {
    Html(include_str!("../web/configure.html"))
}

async fn icons_page_handler() -> Html<&'static str> {
    Html(include_str!("../web/icons.html"))
}

async fn settings_handler() -> Html<&'static str> {
    Html(include_str!("../web/settings.html"))
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    let rx = state.events.subscribe();
    ws.on_upgrade(move |socket| handle_socket(socket, rx))
}

async fn handle_socket(mut socket: WebSocket, mut rx: broadcast::Receiver<KeyEvent>) {
    loop {
        match rx.recv().await {
            Ok(event) => {
                let Ok(text) = serde_json::to_string(&event) else {
                    continue;
                };
                if socket.send(Message::Text(text.into())).await.is_err() {
                    break;
                }
            }
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

#[derive(Debug, Serialize)]
struct KeyConfigView {
    key: u8,
    icon: Option<String>,
    action: Option<Action>,
    protected: bool,
    /// Il tasto fa parte di un pulsante multiplo, che ne decide contenuto e pressione.
    multi: bool,
}

fn key_config_view(config: &Config, key: u8) -> KeyConfigView {
    let existing = config.keys.iter().find(|kc| kc.key == key);
    KeyConfigView {
        key,
        icon: existing.and_then(|kc| kc.icon.clone()),
        action: existing.and_then(|kc| kc.action.clone()),
        protected: existing.is_some_and(|kc| kc.protected),
        multi: config::multi_for_key(config, key).is_some(),
    }
}

async fn list_keys_handler(State(state): State<AppState>) -> Json<Vec<KeyConfigView>> {
    let config = state.config.read().await;
    let views = (1u8..=15).map(|key| key_config_view(&config, key)).collect();
    Json(views)
}

/// File immagine inviati nel campo `icon` di un form multipart, con il nome
/// originale. Ogni file deve essere un'immagine valida.
async fn read_icon_uploads(multipart: &mut Multipart) -> Result<Vec<(String, Vec<u8>)>, (StatusCode, String)> {
    let mut uploads = Vec::new();
    while let Some(field) = multipart.next_field().await.map_err(internal_err)? {
        if field.name() != Some("icon") {
            continue;
        }
        let name = field.file_name().unwrap_or("icona.png").to_string();
        let bytes = field.bytes().await.map_err(internal_err)?.to_vec();
        image::load_from_memory(&bytes).map_err(|e| bad_request(format!("{name}: immagine non valida: {e}")))?;
        uploads.push((name, bytes));
    }
    if uploads.is_empty() {
        return Err(bad_request("campo 'icon' mancante nel form"));
    }
    Ok(uploads)
}

/// Assegna al tasto l'icona `file` della libreria: la scrive subito sul deck
/// e la salva in `config.toml`.
async fn assign_icon(state: &AppState, key: u8, file: &str) -> Result<KeyConfigView, (StatusCode, String)> {
    let image = image::open(state.icons_dir.join(file))
        .map_err(|e| bad_request(format!("impossibile aprire l'icona {file}: {e}")))?;

    let key_icons = icons::prepare_key_icons(image);
    icons::write_icon_to_device(&state.device, key, key_icons.normal.clone())
        .await
        .map_err(internal_err)?;
    state.icon_cache.write().await.insert(key, key_icons);
    // Un tasto luce ridisegna il livello sotto la nuova icona, quello di un
    // pulsante multiplo il suo contenuto al posto dell'icona.
    state.light_wake.notify_one();
    state.multi_wake.notify_one();

    let mut config = state.config.write().await;
    config::set_key_icon(&mut config, key, icon_library::config_path(file));
    config::save(&config, &state.config_path).map_err(internal_err)?;

    Ok(key_config_view(&config, key))
}

/// Carica un'icona nella libreria e la assegna al tasto.
async fn upload_icon_handler(
    State(state): State<AppState>,
    AxumPath(key): AxumPath<u8>,
    mut multipart: Multipart,
) -> Result<Json<KeyConfigView>, (StatusCode, String)> {
    check_key_range(key)?;
    let (name, bytes) = read_icon_uploads(&mut multipart).await?.remove(0);
    let file = icon_library::save(&state.icons_dir, &name, &bytes).map_err(internal_err)?;
    Ok(Json(assign_icon(&state, key, &file).await?))
}

#[derive(Debug, Deserialize)]
struct AssignIconBody {
    /// Nome del file nella libreria (es. `lampada.png`).
    file: String,
}

/// Assegna al tasto un'icona gia' presente nella libreria.
async fn set_icon_handler(
    State(state): State<AppState>,
    AxumPath(key): AxumPath<u8>,
    Json(body): Json<AssignIconBody>,
) -> Result<Json<KeyConfigView>, (StatusCode, String)> {
    check_key_range(key)?;
    if !icon_library::is_library_file(&body.file) {
        return Err(bad_request("nome di icona non valido"));
    }
    Ok(Json(assign_icon(&state, key, &body.file).await?))
}

/// Toglie l'icona di un tasto, lasciando il file nella libreria: il tasto si
/// spegne, salvo che mostri un dato (sistema, luce, pulsante multiplo), che
/// resta disegnato senza icona.
async fn clear_icon_handler(
    State(state): State<AppState>,
    AxumPath(key): AxumPath<u8>,
) -> Result<Json<KeyConfigView>, (StatusCode, String)> {
    check_key_range(key)?;

    let view = {
        let mut config = state.config.write().await;
        config::clear_key_icon(&mut config, key);
        config::save(&config, &state.config_path).map_err(internal_err)?;
        key_config_view(&config, key)
    };

    system::restore_key(&state, key).await;
    state.system_wake.notify_one();
    state.light_wake.notify_one();
    state.multi_wake.notify_one();

    Ok(Json(view))
}

async fn list_icons(state: &AppState) -> Result<Json<Vec<LibraryIcon>>, (StatusCode, String)> {
    let config = state.config.read().await;
    icon_library::list(&state.icons_dir, &config).map(Json).map_err(internal_err)
}

async fn list_icons_handler(State(state): State<AppState>) -> Result<Json<Vec<LibraryIcon>>, (StatusCode, String)> {
    list_icons(&state).await
}

/// Aggiunge alla libreria una o piu' icone, senza assegnarle a un tasto.
async fn upload_library_handler(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Result<Json<Vec<LibraryIcon>>, (StatusCode, String)> {
    for (name, bytes) in read_icon_uploads(&mut multipart).await? {
        icon_library::save(&state.icons_dir, &name, &bytes).map_err(internal_err)?;
    }
    list_icons(&state).await
}

/// Cancella un'icona dalla libreria, se nessun tasto la usa.
async fn delete_library_handler(
    State(state): State<AppState>,
    AxumPath(file): AxumPath<String>,
) -> Result<Json<Vec<LibraryIcon>>, (StatusCode, String)> {
    if !icon_library::is_library_file(&file) {
        return Err(bad_request("nome di icona non valido"));
    }
    {
        let config = state.config.read().await;
        let path = icon_library::config_path(&file);
        let used_by: Vec<String> = config
            .keys
            .iter()
            .filter(|k| k.icon.as_deref() == Some(path.as_str()))
            .map(|k| k.key.to_string())
            .collect();
        if !used_by.is_empty() {
            return Err((
                StatusCode::CONFLICT,
                format!("l'icona e' usata dai tasti {}: toglila prima da li'", used_by.join(", ")),
            ));
        }
        match std::fs::remove_file(state.icons_dir.join(&file)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err((StatusCode::NOT_FOUND, "icona non trovata".to_string()));
            }
            Err(e) => return Err(internal_err(e)),
        }
    }
    list_icons(&state).await
}

fn validate_action(action: &Action) -> Result<(), (StatusCode, String)> {
    match action {
        Action::HomeAssistant { service, entity_id } => {
            if service.split_once('.').is_none() {
                return Err(bad_request(
                    "il campo 'service' deve avere formato dominio.servizio (es. light.toggle)",
                ));
            }
            if entity_id.trim().is_empty() {
                return Err(bad_request("il campo 'entity_id' non puo' essere vuoto"));
            }
        }
        Action::Light { entity_id, step_pct } => {
            if !entity_id.starts_with("light.") {
                return Err(bad_request("entity_id deve essere una luce (es. light.soggiorno)"));
            }
            if !(1..=100).contains(step_pct) {
                return Err(bad_request("il passo di luminosita' deve essere tra 1 e 100"));
            }
        }
        Action::Url { url, method } => {
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                return Err(bad_request(
                    "il campo 'url' deve iniziare con http:// o https://",
                ));
            }
            if let Some(method) = method {
                let valid = ["GET", "POST", "PUT", "PATCH", "DELETE"];
                if !valid.contains(&method.to_uppercase().as_str()) {
                    return Err(bad_request(format!(
                        "metodo HTTP non valido: '{method}' (validi: {})",
                        valid.join(", ")
                    )));
                }
            }
        }
        Action::System { metric } => {
            if system::metric_info(metric).is_none() {
                return Err(bad_request(format!("parametro di sistema sconosciuto: '{metric}'")));
            }
        }
    }
    Ok(())
}

async fn set_action_handler(
    State(state): State<AppState>,
    AxumPath(key): AxumPath<u8>,
    Json(action): Json<Action>,
) -> Result<Json<KeyConfigView>, (StatusCode, String)> {
    check_key_range(key)?;
    validate_action(&action)?;

    let mut config = state.config.write().await;
    if config::multi_for_key(&config, key).is_some() {
        return Err(bad_request(format!(
            "il tasto {key} fa parte di un pulsante multiplo: toglilo prima da li'"
        )));
    }
    config::set_key_action(&mut config, key, action);
    config::save(&config, &state.config_path).map_err(internal_err)?;
    state.system_wake.notify_one();
    state.light_wake.notify_one();

    Ok(Json(key_config_view(&config, key)))
}

#[derive(Debug, Deserialize)]
struct ProtectedBody {
    protected: bool,
}

async fn set_protected_handler(
    State(state): State<AppState>,
    AxumPath(key): AxumPath<u8>,
    Json(body): Json<ProtectedBody>,
) -> Result<Json<KeyConfigView>, (StatusCode, String)> {
    check_key_range(key)?;

    let mut config = state.config.write().await;
    config::set_key_protected(&mut config, key, body.protected);
    config::save(&config, &state.config_path).map_err(internal_err)?;

    Ok(Json(key_config_view(&config, key)))
}

async fn press_key_handler(
    State(state): State<AppState>,
    AxumPath(key): AxumPath<u8>,
) -> Result<StatusCode, (StatusCode, String)> {
    check_key_range(key)?;

    flash_key_icon(&state, key).await;
    run_key_action(&state, key).await;

    Ok(StatusCode::OK)
}

async fn clear_action_handler(
    State(state): State<AppState>,
    AxumPath(key): AxumPath<u8>,
) -> Result<Json<KeyConfigView>, (StatusCode, String)> {
    check_key_range(key)?;

    let mut config = state.config.write().await;
    config::clear_key_action(&mut config, key);
    config::save(&config, &state.config_path).map_err(internal_err)?;
    state.system_wake.notify_one();
    state.light_wake.notify_one();

    Ok(Json(key_config_view(&config, key)))
}

/// Vista delle impostazioni Home Assistant restituita al browser: il token
/// non viene mai rimandato indietro, si indica solo se e' impostato.
#[derive(Debug, Serialize)]
struct HomeAssistantSettingsView {
    url: String,
    token_set: bool,
    verify_tls: bool,
}

impl From<&HomeAssistantConfig> for HomeAssistantSettingsView {
    fn from(ha: &HomeAssistantConfig) -> Self {
        Self {
            url: ha.url.clone(),
            token_set: !ha.token.trim().is_empty(),
            verify_tls: ha.verify_tls,
        }
    }
}

/// Se il form arriva con il token vuoto, riusa quello gia' salvato — ma solo
/// se l'url non cambia, altrimenti chiunque raggiunga l'interfaccia potrebbe
/// puntare l'url verso un proprio server e farsi inviare il token salvato.
fn fill_saved_token(
    body: &mut HomeAssistantConfig,
    saved: &HomeAssistantConfig,
) -> Result<(), String> {
    if !body.token.trim().is_empty() {
        return Ok(());
    }
    if saved.token.trim().is_empty() {
        return Ok(());
    }
    if body.url.trim_end_matches('/') != saved.url.trim_end_matches('/') {
        return Err("hai cambiato l'url: inserisci di nuovo il token".to_string());
    }
    body.token = saved.token.clone();
    Ok(())
}

async fn get_home_assistant_settings(
    State(state): State<AppState>,
) -> Json<HomeAssistantSettingsView> {
    Json((&state.config.read().await.home_assistant).into())
}

async fn set_home_assistant_settings(
    State(state): State<AppState>,
    Json(mut body): Json<HomeAssistantConfig>,
) -> Result<Json<HomeAssistantSettingsView>, (StatusCode, String)> {
    if !body.url.trim().is_empty()
        && !(body.url.starts_with("http://") || body.url.starts_with("https://"))
    {
        return Err(bad_request("il campo 'url' deve iniziare con http:// o https://"));
    }

    let mut config = state.config.write().await;
    fill_saved_token(&mut body, &config.home_assistant).map_err(bad_request)?;
    config.home_assistant = body;
    config::save(&config, &state.config_path).map_err(internal_err)?;

    Ok(Json((&config.home_assistant).into()))
}

async fn list_system_metrics() -> Json<&'static [system::MetricInfo]> {
    Json(system::METRICS)
}

#[derive(Debug, Serialize)]
struct BarView {
    config: Option<BarConfig>,
    status: BarStatus,
}

async fn bar_view(state: &AppState) -> BarView {
    BarView {
        config: state.config.read().await.bar.clone(),
        status: state.bar_status.read().await.clone(),
    }
}

async fn get_bar_handler(State(state): State<AppState>) -> Json<BarView> {
    Json(bar_view(&state).await)
}

async fn set_bar_handler(
    State(state): State<AppState>,
    Json(mut body): Json<BarConfig>,
) -> Result<Json<BarView>, (StatusCode, String)> {
    body.entity_id = body.entity_id.trim().to_string();
    if !body.entity_id.contains('.') {
        return Err(bad_request("entity_id non valido (es. sensor.potenza)"));
    }
    if !body.min.is_finite() || !body.max.is_finite() {
        return Err(bad_request("min e max devono essere numeri"));
    }
    if body.max <= body.min {
        return Err(bad_request("max deve essere maggiore di min"));
    }

    {
        let mut config = state.config.write().await;
        config.bar = Some(body);
        config::save(&config, &state.config_path).map_err(internal_err)?;
    }
    state.bar_wake.notify_one();
    Ok(Json(bar_view(&state).await))
}

async fn clear_bar_handler(
    State(state): State<AppState>,
) -> Result<Json<BarView>, (StatusCode, String)> {
    {
        let mut config = state.config.write().await;
        config.bar = None;
        config::save(&config, &state.config_path).map_err(internal_err)?;
    }
    state.bar_wake.notify_one();
    Ok(Json(bar_view(&state).await))
}

/// Controlla un pulsante multiplo prima di salvarlo in posizione `index`
/// (`None` se e' nuovo): tasti validi e non gia' usati da un altro gruppo.
fn validate_multi(config: &Config, index: Option<usize>, multi: &MultiButton) -> Result<(), (StatusCode, String)> {
    match multi {
        MultiButton::Climate { entity_id, up_key, down_key, step } => {
            if !entity_id.starts_with("climate.") {
                return Err(bad_request("entity_id deve essere un termostato (es. climate.soggiorno)"));
            }
            if up_key == down_key {
                return Err(bad_request("i tasti per alzare e abbassare devono essere diversi"));
            }
            if let Some(step) = step {
                if !(step.is_finite() && *step > 0.0 && *step <= 5.0) {
                    return Err(bad_request("il passo deve essere tra 0,1 e 5 gradi"));
                }
            }
        }
    }
    for key in multi.keys() {
        check_key_range(key)?;
        let taken = config
            .multi
            .iter()
            .enumerate()
            .any(|(i, other)| Some(i) != index && other.keys().contains(&key));
        if taken {
            return Err(bad_request(format!("il tasto {key} fa gia' parte di un altro pulsante multiplo")));
        }
    }
    Ok(())
}

async fn list_multi_handler(State(state): State<AppState>) -> Json<Vec<MultiButton>> {
    Json(state.config.read().await.multi.clone())
}

/// Salva il pulsante multiplo in posizione `index` (in coda se `None`). I
/// tasti che usa perdono l'azione propria, che non verrebbe piu' eseguita.
async fn save_multi(
    state: &AppState,
    index: Option<usize>,
    mut multi: MultiButton,
) -> Result<Json<Vec<MultiButton>>, (StatusCode, String)> {
    let MultiButton::Climate { entity_id, .. } = &mut multi;
    *entity_id = entity_id.trim().to_string();

    let mut config = state.config.write().await;
    if index.is_some_and(|i| i >= config.multi.len()) {
        return Err((StatusCode::NOT_FOUND, "pulsante multiplo non trovato".to_string()));
    }
    validate_multi(&config, index, &multi)?;
    for key in multi.keys() {
        config::clear_key_action(&mut config, key);
    }
    match index {
        Some(i) => config.multi[i] = multi,
        None => config.multi.push(multi),
    }
    config::save(&config, &state.config_path).map_err(internal_err)?;
    state.system_wake.notify_one();
    state.light_wake.notify_one();
    state.multi_wake.notify_one();
    Ok(Json(config.multi.clone()))
}

async fn create_multi_handler(
    State(state): State<AppState>,
    Json(multi): Json<MultiButton>,
) -> Result<Json<Vec<MultiButton>>, (StatusCode, String)> {
    save_multi(&state, None, multi).await
}

async fn update_multi_handler(
    State(state): State<AppState>,
    AxumPath(index): AxumPath<usize>,
    Json(multi): Json<MultiButton>,
) -> Result<Json<Vec<MultiButton>>, (StatusCode, String)> {
    save_multi(&state, Some(index), multi).await
}

async fn delete_multi_handler(
    State(state): State<AppState>,
    AxumPath(index): AxumPath<usize>,
) -> Result<Json<Vec<MultiButton>>, (StatusCode, String)> {
    let mut config = state.config.write().await;
    if index >= config.multi.len() {
        return Err((StatusCode::NOT_FOUND, "pulsante multiplo non trovato".to_string()));
    }
    config.multi.remove(index);
    config::save(&config, &state.config_path).map_err(internal_err)?;
    state.multi_wake.notify_one();
    Ok(Json(config.multi.clone()))
}

#[derive(Debug, Serialize)]
struct TestConnectionResult {
    success: bool,
    message: String,
}

async fn list_home_assistant_entities(
    State(state): State<AppState>,
) -> Result<Json<Vec<actions::EntitySummary>>, (StatusCode, String)> {
    let ha = state.config.read().await.home_assistant.clone();
    let entities = actions::list_entities(&ha).await.map_err(|e| {
        (
            StatusCode::BAD_GATEWAY,
            format!("impossibile recuperare le entita' da Home Assistant: {e}"),
        )
    })?;
    Ok(Json(entities))
}

async fn list_home_assistant_services(
    State(state): State<AppState>,
) -> Result<Json<Vec<actions::DomainServices>>, (StatusCode, String)> {
    let ha = state.config.read().await.home_assistant.clone();
    let services = actions::list_services(&ha).await.map_err(|e| {
        (
            StatusCode::BAD_GATEWAY,
            format!("impossibile recuperare i servizi da Home Assistant: {e}"),
        )
    })?;
    Ok(Json(services))
}

async fn test_home_assistant_settings(
    State(state): State<AppState>,
    Json(mut body): Json<HomeAssistantConfig>,
) -> Json<TestConnectionResult> {
    let saved = state.config.read().await.home_assistant.clone();
    if let Err(message) = fill_saved_token(&mut body, &saved) {
        return Json(TestConnectionResult {
            success: false,
            message,
        });
    }
    match actions::test_home_assistant_connection(&body).await {
        Ok(message) => Json(TestConnectionResult {
            success: true,
            message,
        }),
        Err(e) => Json(TestConnectionResult {
            success: false,
            message: e.to_string(),
        }),
    }
}

fn router(state: AppState) -> Router {
    let icons_dir = (*state.icons_dir).clone();

    // Rotte raggiungibili senza sessione: solo quelle necessarie per creare
    // la password o fare login.
    let public = Router::new()
        .route("/login", get(auth::login_page))
        .route("/api/auth/status", get(auth::status_handler))
        .route("/api/auth/setup", axum::routing::post(auth::setup_handler))
        .route("/api/auth/login", axum::routing::post(auth::login_handler));

    Router::new()
        .route("/", get(index_handler))
        .route("/configura", get(configure_handler))
        .route("/impostazioni", get(settings_handler))
        .route("/icone", get(icons_page_handler))
        .route("/api/status", get(status_handler))
        .route("/ws", get(ws_handler))
        .route(
            "/api/keys",
            get(list_keys_handler),
        )
        .route(
            "/api/keys/{key}/icon",
            axum::routing::post(upload_icon_handler).put(set_icon_handler).delete(clear_icon_handler),
        )
        .route(
            "/api/keys/{key}/action",
            axum::routing::put(set_action_handler).delete(clear_action_handler),
        )
        .route(
            "/api/keys/{key}/protected",
            axum::routing::put(set_protected_handler),
        )
        .route(
            "/api/keys/{key}/press",
            axum::routing::post(press_key_handler),
        )
        .route(
            "/api/settings/home_assistant",
            get(get_home_assistant_settings).put(set_home_assistant_settings),
        )
        .route(
            "/api/settings/home_assistant/test",
            axum::routing::post(test_home_assistant_settings),
        )
        .route(
            "/api/home_assistant/entities",
            get(list_home_assistant_entities),
        )
        .route(
            "/api/home_assistant/services",
            get(list_home_assistant_services),
        )
        .route("/api/system/metrics", get(list_system_metrics))
        .route(
            "/api/icons",
            get(list_icons_handler).post(upload_library_handler),
        )
        .route(
            "/api/icons/{file}",
            axum::routing::delete(delete_library_handler),
        )
        .route(
            "/api/multi",
            get(list_multi_handler).post(create_multi_handler),
        )
        .route(
            "/api/multi/{index}",
            axum::routing::put(update_multi_handler).delete(delete_multi_handler),
        )
        .route(
            "/api/bar",
            get(get_bar_handler).put(set_bar_handler).delete(clear_bar_handler),
        )
        .route("/api/auth/logout", axum::routing::post(auth::logout_handler))
        .route(
            "/api/auth/password",
            axum::routing::post(auth::change_password_handler),
        )
        .nest_service("/icons", tower_http::services::ServeDir::new(icons_dir))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::require_session,
        ))
        .merge(public)
        .with_state(state)
}

pub async fn serve(state: AppState, addr: SocketAddr) -> anyhow::Result<()> {
    let app = router(state);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("Interfaccia web in ascolto su http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
}
