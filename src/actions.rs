use crate::config::{Action, HomeAssistantConfig};

pub async fn execute(
    client: &reqwest::Client,
    ha: &HomeAssistantConfig,
    action: &Action,
) -> anyhow::Result<()> {
    match action {
        Action::HomeAssistant {
            service,
            entity_id,
        } => {
            let (domain, service_name) = service.split_once('.').ok_or_else(|| {
                anyhow::anyhow!("service non valido: '{service}' (atteso formato dominio.servizio)")
            })?;

            let url = format!(
                "{}/api/services/{domain}/{service_name}",
                ha.url.trim_end_matches('/')
            );

            let response = client
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
