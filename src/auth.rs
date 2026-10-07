//! Login con password e sessioni via cookie per l'interfaccia web.
//!
//! La password e' salvata solo come hash argon2 in `config.toml`
//! (`[web] password_hash`). Finche' non e' impostata, l'interfaccia mostra
//! solo la pagina di creazione password; per reimpostarla basta cancellare
//! quella riga e riavviare il servizio. Le sessioni vivono in memoria: un
//! riavvio richiede un nuovo login.

use argon2::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use rand_core::{OsRng, RngCore};
use axum::{
    extract::{Request, State},
    http::{header, HeaderMap, Method, StatusCode},
    middleware::Next,
    response::{Html, IntoResponse, Redirect, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock};

use crate::config;
use crate::web::AppState;

const COOKIE_NAME: &str = "hassdeck_session";
const SESSION_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const MIN_PASSWORD_LEN: usize = 8;
/// Attesa dopo un tentativo di login fallito. I tentativi sono serializzati
/// (vedi `Auth::login_lock`), quindi limita il brute force a circa un
/// tentativo al secondo complessivo.
const FAILED_LOGIN_DELAY: Duration = Duration::from_secs(1);

#[derive(Clone, Default)]
pub struct Auth {
    sessions: Arc<RwLock<HashMap<String, Instant>>>,
    login_lock: Arc<Mutex<()>>,
}

impl Auth {
    async fn create_session(&self) -> String {
        let mut bytes = [0u8; 32];
        OsRng.fill_bytes(&mut bytes);
        let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();

        let mut sessions = self.sessions.write().await;
        let now = Instant::now();
        sessions.retain(|_, expires| *expires > now);
        sessions.insert(token.clone(), now + SESSION_TTL);
        token
    }

    async fn is_valid(&self, token: &str) -> bool {
        self.sessions
            .read()
            .await
            .get(token)
            .is_some_and(|expires| *expires > Instant::now())
    }

    async fn remove(&self, token: &str) {
        self.sessions.write().await.remove(token);
    }

    async fn clear_all(&self) {
        self.sessions.write().await.clear();
    }
}

fn hash_password(password: &str) -> anyhow::Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| anyhow::anyhow!("impossibile calcolare l'hash della password: {e}"))
}

fn verify_password(password: &str, hash: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

fn session_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == COOKIE_NAME)
        .map(|(_, value)| value.to_string())
}

fn session_cookie(token: &str) -> String {
    format!(
        "{COOKIE_NAME}={token}; HttpOnly; SameSite=Strict; Path=/; Max-Age={}",
        SESSION_TTL.as_secs()
    )
}

fn expired_cookie() -> String {
    format!("{COOKIE_NAME}=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0")
}

fn with_session_cookie(token: &str) -> Response {
    (
        StatusCode::OK,
        [(header::SET_COOKIE, session_cookie(token))],
    )
        .into_response()
}

fn bad_request(msg: impl Into<String>) -> (StatusCode, String) {
    (StatusCode::BAD_REQUEST, msg.into())
}

fn internal_err<E: std::fmt::Display>(e: E) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

fn check_password_strength(password: &str) -> Result<(), (StatusCode, String)> {
    if password.chars().count() < MIN_PASSWORD_LEN {
        return Err(bad_request(format!(
            "la password deve avere almeno {MIN_PASSWORD_LEN} caratteri"
        )));
    }
    Ok(())
}

/// Middleware che protegge tutte le rotte tranne quelle di login: le pagine
/// HTML reindirizzano a `/login`, le API rispondono 401.
pub async fn require_session(State(state): State<AppState>, request: Request, next: Next) -> Response {
    if let Some(token) = session_token(request.headers()) {
        if state.auth.is_valid(&token).await {
            return next.run(request).await;
        }
    }

    let path = request.uri().path();
    let is_page = request.method() == Method::GET && !path.starts_with("/api/") && path != "/ws";
    if is_page {
        Redirect::to("/login").into_response()
    } else {
        (StatusCode::UNAUTHORIZED, "login richiesto").into_response()
    }
}

pub async fn login_page() -> Html<&'static str> {
    Html(include_str!("../web/login.html"))
}

#[derive(Debug, Serialize)]
pub struct AuthStatus {
    password_set: bool,
}

pub async fn status_handler(State(state): State<AppState>) -> Json<AuthStatus> {
    let password_set = state.config.read().await.web.password_hash.is_some();
    Json(AuthStatus { password_set })
}

#[derive(Debug, Deserialize)]
pub struct PasswordBody {
    password: String,
}

/// Crea la password al primo avvio. Rifiutato se una password esiste gia'.
pub async fn setup_handler(
    State(state): State<AppState>,
    Json(body): Json<PasswordBody>,
) -> Result<Response, (StatusCode, String)> {
    check_password_strength(&body.password)?;

    let mut config = state.config.write().await;
    if config.web.password_hash.is_some() {
        return Err((StatusCode::CONFLICT, "password gia' impostata".to_string()));
    }
    config.web.password_hash = Some(hash_password(&body.password).map_err(internal_err)?);
    config::save(&config, &state.config_path).map_err(internal_err)?;
    drop(config);

    let token = state.auth.create_session().await;
    Ok(with_session_cookie(&token))
}

pub async fn login_handler(
    State(state): State<AppState>,
    Json(body): Json<PasswordBody>,
) -> Result<Response, (StatusCode, String)> {
    let _guard = state.auth.login_lock.lock().await;

    let hash = state.config.read().await.web.password_hash.clone();
    let Some(hash) = hash else {
        return Err((StatusCode::CONFLICT, "nessuna password impostata".to_string()));
    };

    if !verify_password(&body.password, &hash) {
        tokio::time::sleep(FAILED_LOGIN_DELAY).await;
        return Err((StatusCode::UNAUTHORIZED, "password errata".to_string()));
    }

    let token = state.auth.create_session().await;
    Ok(with_session_cookie(&token))
}

pub async fn logout_handler(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(token) = session_token(&headers) {
        state.auth.remove(&token).await;
    }
    (StatusCode::OK, [(header::SET_COOKIE, expired_cookie())]).into_response()
}

#[derive(Debug, Deserialize)]
pub struct ChangePasswordBody {
    current: String,
    new: String,
}

/// Cambia la password e chiude tutte le sessioni, tranne quella corrente
/// che viene sostituita da una nuova.
pub async fn change_password_handler(
    State(state): State<AppState>,
    Json(body): Json<ChangePasswordBody>,
) -> Result<Response, (StatusCode, String)> {
    let _guard = state.auth.login_lock.lock().await;
    check_password_strength(&body.new)?;

    let mut config = state.config.write().await;
    let current_ok = config
        .web
        .password_hash
        .as_deref()
        .is_some_and(|hash| verify_password(&body.current, hash));
    if !current_ok {
        drop(config);
        tokio::time::sleep(FAILED_LOGIN_DELAY).await;
        return Err((StatusCode::FORBIDDEN, "password attuale errata".to_string()));
    }

    config.web.password_hash = Some(hash_password(&body.new).map_err(internal_err)?);
    config::save(&config, &state.config_path).map_err(internal_err)?;
    drop(config);

    state.auth.clear_all().await;
    let token = state.auth.create_session().await;
    Ok(with_session_cookie(&token))
}
