use crate::config::{Action, HomeAssistantConfig};

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
