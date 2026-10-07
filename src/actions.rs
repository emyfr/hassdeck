use crate::config::{Action, HomeAssistantConfig};
use serde::Serialize;

#[derive(Debug, Serialize, Clone)]
pub struct EntitySummary {
    pub entity_id: String,
    pub friendly_name: Option<String>,
    /// Presente per i sensori numerici, usato dall'editor per proporre i
    /// sensori adatti alla barra verticale.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit_of_measurement: Option<String>,
}

/// Stato corrente di un'entita' (`state` e' sempre una stringa, anche per i
/// sensori numerici, e puo' valere "unavailable" o "unknown").
#[derive(Debug, Clone)]
pub struct EntityState {
    pub state: String,
    pub unit: Option<String>,
}

fn build_home_assistant_client(ha: &HomeAssistantConfig) -> anyhow::Result<reqwest::Client> {
    reqwest::Client::builder()
        .danger_accept_invalid_certs(!ha.verify_tls)
        .build()
        .map_err(|e| anyhow::anyhow!("impossibile creare il client HTTP per Home Assistant: {e}"))
}

/// Verifica la connessione a Home Assistant chiamando l'endpoint base
/// dell'API. Ritorna il messaggio riportato da Home Assistant in caso di
/// successo (es. "API running."), o un errore descrittivo altrimenti.
pub async fn test_home_assistant_connection(ha: &HomeAssistantConfig) -> anyhow::Result<String> {
    if ha.url.trim().is_empty() {
        anyhow::bail!("url non impostato");
    }
    if ha.token.trim().is_empty() {
        anyhow::bail!("token non impostato");
    }

    let client = build_home_assistant_client(ha)?;
    let url = format!("{}/api/", ha.url.trim_end_matches('/'));

    let response = client
        .get(&url)
        .bearer_auth(&ha.token)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("impossibile contattare {url}: {e}"))?;

    let status = response.status();
    let body = response.text().await.unwrap_or_default();

    if !status.is_success() {
        anyhow::bail!("Home Assistant ha risposto {status}: {body}");
    }

    Ok(body)
}

/// Recupera l'elenco delle entita' esposte da Home Assistant (`GET
/// /api/states`), per popolare un menu a tendina nell'editor invece di
/// dover digitare a mano gli `entity_id`.
pub async fn list_entities(ha: &HomeAssistantConfig) -> anyhow::Result<Vec<EntitySummary>> {
    if ha.url.trim().is_empty() || ha.token.trim().is_empty() {
        anyhow::bail!("url o token Home Assistant non impostati (vedi /impostazioni)");
    }

    let client = build_home_assistant_client(ha)?;
    let url = format!("{}/api/states", ha.url.trim_end_matches('/'));

    let response = client
        .get(&url)
        .bearer_auth(&ha.token)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("impossibile contattare {url}: {e}"))?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Home Assistant ha risposto {status}: {body}");
    }

    let states: Vec<serde_json::Value> = response
        .json()
        .await
        .map_err(|e| anyhow::anyhow!("risposta di Home Assistant non valida: {e}"))?;

    let mut entities: Vec<EntitySummary> = states
        .into_iter()
        .filter_map(|state| {
            let entity_id = state.get("entity_id")?.as_str()?.to_string();
            let attribute = |name: &str| {
                state
                    .get("attributes")
                    .and_then(|a| a.get(name))
                    .and_then(|n| n.as_str())
                    .map(str::to_string)
            };
            Some(EntitySummary {
                entity_id,
                friendly_name: attribute("friendly_name"),
                unit_of_measurement: attribute("unit_of_measurement"),
            })
        })
        .collect();

    entities.sort_by(|a, b| a.entity_id.cmp(&b.entity_id));

    Ok(entities)
}

/// Legge lo stato di una singola entita' (`GET /api/states/<entity_id>`).
pub async fn get_state(ha: &HomeAssistantConfig, entity_id: &str) -> anyhow::Result<EntityState> {
    if ha.url.trim().is_empty() || ha.token.trim().is_empty() {
        anyhow::bail!("url o token Home Assistant non impostati (vedi /impostazioni)");
    }

    let client = build_home_assistant_client(ha)?;
    let url = format!("{}/api/states/{entity_id}", ha.url.trim_end_matches('/'));

    let response = client
        .get(&url)
        .bearer_auth(&ha.token)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("impossibile contattare {url}: {e}"))?;

    let status = response.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        anyhow::bail!("entita' {entity_id} non trovata in Home Assistant");
    }
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Home Assistant ha risposto {status}: {body}");
    }

    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| anyhow::anyhow!("risposta di Home Assistant non valida: {e}"))?;

    Ok(EntityState {
        state: body
            .get("state")
            .and_then(|s| s.as_str())
            .unwrap_or_default()
            .to_string(),
        unit: body
            .get("attributes")
            .and_then(|a| a.get("unit_of_measurement"))
            .and_then(|u| u.as_str())
            .map(str::to_string),
    })
}

#[derive(Debug, Serialize, Clone)]
pub struct ServiceSummary {
    pub service: String,
    pub name: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
pub struct DomainServices {
    pub domain: String,
    pub services: Vec<ServiceSummary>,
}

/// Recupera l'elenco dei servizi esposti da Home Assistant (`GET
/// /api/services`), raggruppati per dominio, per popolare un menu a tendina
/// filtrato in base al dominio dell'entità selezionata invece di dover
/// scrivere a mano la stringa "dominio.servizio".
pub async fn list_services(ha: &HomeAssistantConfig) -> anyhow::Result<Vec<DomainServices>> {
    if ha.url.trim().is_empty() || ha.token.trim().is_empty() {
        anyhow::bail!("url o token Home Assistant non impostati (vedi /impostazioni)");
    }

    let client = build_home_assistant_client(ha)?;
    let url = format!("{}/api/services", ha.url.trim_end_matches('/'));

    let response = client
        .get(&url)
        .bearer_auth(&ha.token)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("impossibile contattare {url}: {e}"))?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Home Assistant ha risposto {status}: {body}");
    }

    let raw: Vec<serde_json::Value> = response
        .json()
        .await
        .map_err(|e| anyhow::anyhow!("risposta di Home Assistant non valida: {e}"))?;

    let mut domains: Vec<DomainServices> = raw
        .into_iter()
        .filter_map(|entry| {
            let domain = entry.get("domain")?.as_str()?.to_string();
            let services_obj = entry.get("services")?.as_object()?;
            let mut services: Vec<ServiceSummary> = services_obj
                .iter()
                .map(|(service, meta)| ServiceSummary {
                    service: service.clone(),
                    name: meta.get("name").and_then(|n| n.as_str()).map(str::to_string),
                })
                .collect();
            services.sort_by(|a, b| a.service.cmp(&b.service));
            Some(DomainServices { domain, services })
        })
        .collect();

    domains.sort_by(|a, b| a.domain.cmp(&b.domain));

    Ok(domains)
}

pub async fn execute(
    client: &reqwest::Client,
    ha: &HomeAssistantConfig,
    action: &Action,
) -> anyhow::Result<()> {
    match action {
        Action::HomeAssistant { service, entity_id } => {
            let (domain, service_name) = service.split_once('.').ok_or_else(|| {
                anyhow::anyhow!("service non valido: '{service}' (atteso formato dominio.servizio)")
            })?;

            let url = format!(
                "{}/api/services/{domain}/{service_name}",
                ha.url.trim_end_matches('/')
            );

            let ha_client = build_home_assistant_client(ha)?;
            let response = ha_client
                .post(&url)
                .bearer_auth(&ha.token)
                .json(&serde_json::json!({ "entity_id": entity_id }))
                .send()
                .await?;

            if !response.status().is_success() {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                anyhow::bail!("Home Assistant ha risposto {status}: {body}");
            }

            Ok(())
        }

        Action::Url { url, method } => {
            let method_name = method.as_deref().unwrap_or("POST");
            let method: reqwest::Method = method_name
                .parse()
                .map_err(|_| anyhow::anyhow!("metodo HTTP non valido: '{method_name}'"))?;

            let response = client.request(method, url).send().await?;

            if !response.status().is_success() {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                anyhow::bail!("La chiamata a {url} ha risposto {status}: {body}");
            }

            Ok(())
        }
    }
}
