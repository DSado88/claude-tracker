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
        .header("User-Agent", "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.3 Safari/605.1.15")
        .header("Referer", "https://claude.ai/")
        .timeout(Duration::from_secs(10))
        .send()
        .await?
        .error_for_status()?;

    let body: serde_json::Value = resp.json().await?;

    let five_hour = body
        .get("five_hour")
        .ok_or_else(|| anyhow::anyhow!("Missing five_hour field"))?;

    let utilization = parse_utilization(five_hour);
    let resets_at = parse_resets_at(five_hour);

    // Parse seven_day bucket (may be null)
    let (weekly_utilization, weekly_resets_at) = body
        .get("seven_day")
        .filter(|v| !v.is_null())
        .map(|seven_day| (Some(parse_utilization(seven_day)), parse_resets_at(seven_day)))
        .unwrap_or((None, None));

    Ok(UsageData {
        utilization,
        resets_at,
        weekly_utilization,
        weekly_resets_at,
    })
}

fn parse_utilization(bucket: &serde_json::Value) -> u32 {
    bucket
        .get("utilization")
        .and_then(|v| v.as_u64().map(|n| n as f64).or_else(|| v.as_f64()))
        .map(|v| {
            if v > 0.0 && v <= 1.0 {
                (v * 100.0).round() as u32
            } else {
                v.round() as u32
            }
        })
        .unwrap_or(0)
}

fn parse_resets_at(bucket: &serde_json::Value) -> Option<DateTime<Utc>> {
    bucket
        .get("resets_at")
        .and_then(|v| v.as_str())
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc))
}
