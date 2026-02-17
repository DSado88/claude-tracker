use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::sync::mpsc;

use crate::app::{AppState, UsageData};
use crate::event::Event;

pub fn spawn_fetch_all(app: &AppState, tx: &mpsc::UnboundedSender<Event>) {
    for (i, account) in app.accounts.iter().enumerate() {
        let tx = tx.clone();
        let account_name = account.config.name.clone();
        let org_id = account.config.org_id.clone();
        let keyring = app.keyring.clone();
        let stagger = Duration::from_millis(100 * i as u64);

        tokio::spawn(async move {
            tokio::time::sleep(stagger).await;

            let session_key = match keyring.get_session_key(&account_name) {
                Ok(key) => key,
                Err(e) => {
                    let _ = tx.send(Event::UsageResult {
                        account_name: account_name.clone(),
                        result: Err(e.to_string()),
                    });
                    return;
                }
            };

            let result = fetch_usage(&session_key, &org_id).await;
            let _ = tx.send(Event::UsageResult {
                account_name: account_name.clone(),
                result: result.map_err(|e| e.to_string()),
            });
        });
    }
}

pub fn spawn_fetch_one(
    app: &AppState,
    index: usize,
    tx: &mpsc::UnboundedSender<Event>,
) {
    if let Some(account) = app.accounts.get(index) {
        let tx = tx.clone();
        let account_name = account.config.name.clone();
        let org_id = account.config.org_id.clone();
        let keyring = app.keyring.clone();

        tokio::spawn(async move {
            let session_key = match keyring.get_session_key(&account_name) {
                Ok(key) => key,
                Err(e) => {
                    let _ = tx.send(Event::UsageResult {
                        account_name: account_name.clone(),
                        result: Err(e.to_string()),
                    });
                    return;
                }
            };

            let result = fetch_usage(&session_key, &org_id).await;
            let _ = tx.send(Event::UsageResult {
                account_name: account_name.clone(),
                result: result.map_err(|e| e.to_string()),
            });
        });
    }
}

async fn fetch_usage(session_key: &str, org_id: &str) -> anyhow::Result<UsageData> {
    let client = reqwest::Client::new();
    let url = format!(
        "https://claude.ai/api/organizations/{}/usage",
        org_id
    );

    let resp = client
        .get(&url)
        .header("Cookie", format!("sessionKey={}", session_key))
        .header("Accept", "application/json")
        .timeout(Duration::from_secs(10))
        .send()
        .await?
        .error_for_status()?;

    let body: serde_json::Value = resp.json().await?;

    let five_hour = body
        .get("five_hour")
        .ok_or_else(|| anyhow::anyhow!("Missing five_hour field"))?;

    let utilization = five_hour
        .get("utilization")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;

    let resets_at: Option<DateTime<Utc>> = five_hour
        .get("resets_at")
        .and_then(|v| v.as_str())
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc));

    Ok(UsageData {
        utilization,
        resets_at,
    })
}
