use axum::{extract::State, response::Html, routing::get, Json, Router};
use serde::Serialize;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;

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

pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

async fn status_handler(State(status): State<SharedStatus>) -> Json<Status> {
    Json(status.read().await.clone())
}

async fn index_handler() -> Html<&'static str> {
    Html(include_str!("../web/index.html"))
}

fn router(status: SharedStatus) -> Router {
    Router::new()
        .route("/", get(index_handler))
        .route("/api/status", get(status_handler))
        .with_state(status)
}

pub async fn serve(status: SharedStatus, addr: SocketAddr) -> anyhow::Result<()> {
    let app = router(status);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("Interfaccia web in ascolto su http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
}
