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
use serde::Serialize;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::{broadcast, RwLock};

use crate::config::{self, Action, Config};
use crate::icons;
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
/// per animare la griglia dei tasti nella pagina di monitoraggio.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum KeyEvent {
    Key { physical_key: u8, pressed: bool },
    ActionResult { physical_key: u8, success: bool },
}

#[derive(Clone)]
pub struct AppState {
    pub status: SharedStatus,
    pub events: broadcast::Sender<KeyEvent>,
    pub device: Arc<Device>,
    pub config: SharedConfig,
    pub config_path: Arc<PathBuf>,
    pub icons_dir: Arc<PathBuf>,
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
}

fn key_config_view(config: &Config, key: u8) -> KeyConfigView {
    let existing = config.keys.iter().find(|kc| kc.key == key);
    KeyConfigView {
        key,
        icon: existing.and_then(|kc| kc.icon.clone()),
        action: existing.and_then(|kc| kc.action.clone()),
    }
}

async fn list_keys_handler(State(state): State<AppState>) -> Json<Vec<KeyConfigView>> {
    let config = state.config.read().await;
    let views = (1u8..=15).map(|key| key_config_view(&config, key)).collect();
    Json(views)
}

async fn upload_icon_handler(
    State(state): State<AppState>,
    AxumPath(key): AxumPath<u8>,
    mut multipart: Multipart,
) -> Result<Json<KeyConfigView>, (StatusCode, String)> {
    check_key_range(key)?;

    let mut bytes: Option<Vec<u8>> = None;
    let mut extension = "png".to_string();

    while let Some(field) = multipart.next_field().await.map_err(internal_err)? {
        if field.name() != Some("icon") {
            continue;
        }
        if let Some(file_name) = field.file_name() {
            if let Some(ext) = file_name.rsplit('.').next() {
                extension = ext.to_lowercase();
            }
        }
        bytes = Some(field.bytes().await.map_err(internal_err)?.to_vec());
    }

    let bytes = bytes.ok_or_else(|| bad_request("campo 'icon' mancante nel form"))?;

    let image = image::load_from_memory(&bytes)
        .map_err(|e| bad_request(format!("immagine non valida: {e}")))?;

    std::fs::create_dir_all(&*state.icons_dir).map_err(internal_err)?;
    let filename = format!("key{key}.{extension}");
    let file_path = state.icons_dir.join(&filename);
    std::fs::write(&file_path, &bytes).map_err(internal_err)?;
    let icon_rel_path = format!("icons/{filename}");

    icons::write_icon_to_device(&state.device, key, image)
        .await
        .map_err(internal_err)?;

    let mut config = state.config.write().await;
    config::set_key_icon(&mut config, key, icon_rel_path);
    config::save(&config, &state.config_path).map_err(internal_err)?;

    Ok(Json(key_config_view(&config, key)))
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
    config::set_key_action(&mut config, key, action);
    config::save(&config, &state.config_path).map_err(internal_err)?;

    Ok(Json(key_config_view(&config, key)))
}

async fn clear_action_handler(
    State(state): State<AppState>,
    AxumPath(key): AxumPath<u8>,
) -> Result<Json<KeyConfigView>, (StatusCode, String)> {
    check_key_range(key)?;

    let mut config = state.config.write().await;
    config::clear_key_action(&mut config, key);
    config::save(&config, &state.config_path).map_err(internal_err)?;

    Ok(Json(key_config_view(&config, key)))
}

fn router(state: AppState) -> Router {
    let icons_dir = (*state.icons_dir).clone();
    Router::new()
        .route("/", get(index_handler))
        .route("/configura", get(configure_handler))
        .route("/api/status", get(status_handler))
        .route("/ws", get(ws_handler))
        .route(
            "/api/keys",
            get(list_keys_handler),
        )
        .route(
            "/api/keys/{key}/icon",
            axum::routing::post(upload_icon_handler),
        )
        .route(
            "/api/keys/{key}/action",
            axum::routing::put(set_action_handler).delete(clear_action_handler),
        )
        .nest_service("/icons", tower_http::services::ServeDir::new(icons_dir))
        .with_state(state)
}

pub async fn serve(state: AppState, addr: SocketAddr) -> anyhow::Result<()> {
    let app = router(state);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("Interfaccia web in ascolto su http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
}
