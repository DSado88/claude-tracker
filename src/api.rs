use std::sync::OnceLock;
use std::time::Duration;

use tokio::sync::mpsc;

use crate::app::{AppState, UsageData};
use crate::config::AuthMethod;
use crate::event::Event;
use crate::oauth;

/// Shared HTTP client for connection pooling across all API calls.
pub(crate) fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(reqwest::Client::new)
}

pub fn spawn_fetch_all(app: &AppState, tx: &mpsc::UnboundedSender<Event>) {
    for (i, account) in app.accounts.iter().enumerate() {
        let tx = tx.clone();
        let account_name = account.config.name.clone();
        let org_id = account.config.org_id.clone();
        let auth_method = account.config.auth_method.clone();
        let cached_token = account.cached_token.clone();
        let stagger = Duration::from_millis(100 * i as u64);

        tokio::spawn(async move {
            tokio::time::sleep(stagger).await;
            let result = fetch_account_usage(&org_id, &auth_method, cached_token.as_deref()).await;
            let _ = tx.send(Event::UsageResult {
                account_name,
                result,
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
        let auth_method = account.config.auth_method.clone();
        let cached_token = account.cached_token.clone();

        tokio::spawn(async move {
            let result = fetch_account_usage(&org_id, &auth_method, cached_token.as_deref()).await;
            let _ = tx.send(Event::UsageResult {
                account_name,
                result,
            });
        });
    }
}

/// Shared fetch logic. Uses cached token from memory — no keychain reads.
async fn fetch_account_usage(
    org_id: &str,
    auth_method: &AuthMethod,
    cached_token: Option<&str>,
) -> Result<UsageData, String> {
    let token = cached_token
        .ok_or_else(|| "No token cached — re-import (i)".to_string())?;
    let result = match auth_method {
        AuthMethod::SessionKey => {
            fetch_usage_session_key(token, org_id).await
        }
        AuthMethod::OAuth => {
            let normalized = oauth::normalize_stored_token(token);
            oauth::fetch_oauth_usage(&normalized).await
        }
    };
    result.map_err(|e| humanize_error(&e))
}

/// Turn common API errors into short, actionable messages.
fn humanize_error(e: &anyhow::Error) -> String {
    let msg = format!("{e:#}");
    if msg.contains("401") || msg.contains("403") {
        "Expired — re-import (i)".to_string()
    } else if msg.contains("429") {
        "Rate limited — try later".to_string()
    } else if msg.contains("timed out") || msg.contains("timeout") {
        "Timeout".to_string()
    } else if msg.contains("connect") || msg.contains("dns") || msg.contains("resolve") {
        "No network".to_string()
    } else {
        msg
    }
}

/// Import OAuth credentials from Claude Code's keychain, identify the account,
/// and send the result back.
pub fn spawn_oauth_import(tx: &mpsc::UnboundedSender<Event>) {
    let tx = tx.clone();
    tokio::spawn(async move {
        let result = do_oauth_import().await;
        let _ = tx.send(Event::OAuthImportResult {
            result: result.map_err(|e| format!("{e:#}")),
        });
    });
}

async fn do_oauth_import() -> anyhow::Result<crate::event::OAuthImportData> {
    // Read Claude Code's access token from macOS Keychain
    let access_token = oauth::read_claude_code_access_token()?;

    // Fetch profile to identify the account (also validates the token is alive)
    let profile = oauth::fetch_profile(&access_token).await?;

    Ok(crate::event::OAuthImportData {
        name: profile.email,
        org_id: profile.org_id,
        access_token,
    })
}

/// Detect which account matches the token currently in Claude Code's keychain.
/// Compares cached tokens in memory — only one `security` CLI call for Claude Code's keychain.
pub fn spawn_detect_logged_in(app: &AppState, tx: &mpsc::UnboundedSender<Event>) {
    let tx = tx.clone();
    let oauth_accounts: Vec<(String, String)> = app
        .accounts
        .iter()
        .filter(|a| a.config.auth_method == AuthMethod::OAuth)
        .filter_map(|a| {
            a.cached_token.as_ref().map(|t| {
                (a.config.name.clone(), oauth::normalize_stored_token(t))
            })
        })
        .collect();

    tokio::spawn(async move {
        let result = tokio::task::spawn_blocking(move || {
            let cc_token = oauth::read_claude_code_access_token().ok()?;
            for (name, token) in &oauth_accounts {
                if *token == cc_token {
                    return Some(name.clone());
                }
            }
            None
        })
        .await
        .unwrap_or_else(|e| {
            eprintln!("detect_logged_in task panicked: {e}");
            None
        });

        let _ = tx.send(Event::LoggedInDetected {
            account_name: result,
        });
    });
}

async fn fetch_usage_session_key(session_key: &str, org_id: &str) -> anyhow::Result<UsageData> {
    let client = http_client();
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

    let utilization = oauth::parse_utilization(five_hour);
    let resets_at = oauth::parse_resets_at(five_hour);

    let (weekly_utilization, weekly_resets_at) = body
        .get("seven_day")
        .filter(|v| !v.is_null())
        .map(|seven_day| (Some(oauth::parse_utilization(seven_day)), oauth::parse_resets_at(seven_day)))
        .unwrap_or((None, None));

    Ok(UsageData {
        utilization,
        resets_at,
        weekly_utilization,
        weekly_resets_at,
    })
}
