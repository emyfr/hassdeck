use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::{Html, IntoResponse},
    routing::get,
    Json, Router,
};
use serde::Serialize;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::{broadcast, RwLock};

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
}

pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

async fn status_handler(State(state): State<AppState>) -> Json<Status> {
    Json(state.status.read().await.clone())
}

async fn index_handler() -> Html<&'static str> {
    Html(include_str!("../web/index.html"))
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

fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index_handler))
        .route("/api/status", get(status_handler))
        .route("/ws", get(ws_handler))
        .with_state(state)
}

pub async fn serve(state: AppState, addr: SocketAddr) -> anyhow::Result<()> {
    let app = router(state);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("Interfaccia web in ascolto su http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
}
