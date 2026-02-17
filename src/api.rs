use std::sync::{Arc, OnceLock};
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
        let keyring = app.keyring.clone();
        let stagger = Duration::from_millis(100 * i as u64);

        tokio::spawn(async move {
            tokio::time::sleep(stagger).await;
            let result = fetch_account_usage(&account_name, &org_id, &auth_method, &keyring).await;
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
        let keyring = app.keyring.clone();

        tokio::spawn(async move {
            let result = fetch_account_usage(&account_name, &org_id, &auth_method, &keyring).await;
            let _ = tx.send(Event::UsageResult {
                account_name,
                result,
            });
        });
    }
}

/// Shared fetch logic for both spawn_fetch_all and spawn_fetch_one.
/// Uses format!("{e:#}") to preserve the full anyhow error chain across the channel boundary.
async fn fetch_account_usage(
    account_name: &str,
    org_id: &str,
    auth_method: &AuthMethod,
    keyring: &Arc<dyn crate::keyring_store::KeyringBackend>,
) -> Result<UsageData, String> {
    let result = match auth_method {
        AuthMethod::SessionKey => {
            let session_key = keyring
                .get_session_key(account_name)
                .map_err(|e| format!("{e:#}"))?;
            fetch_usage_session_key(&session_key, org_id).await
        }
        AuthMethod::OAuth => {
            let token = oauth::get_stored_token(keyring.as_ref(), account_name)
                .map_err(|e| format!("{e:#}"))?;
            oauth::fetch_oauth_usage(&token).await
        }
    };
    result.map_err(|e| format!("{e:#}"))
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
    // Read Claude Code's credentials from macOS Keychain
    let cred = oauth::read_claude_code_keychain()?;

    // We don't refresh tokens ourselves to avoid token stripping detection.
    if cred.needs_refresh() {
        return Err(anyhow::anyhow!(
            "Token expired. Use Claude Code first (any command), then press 'i' again"
        ));
    }

    // Fetch profile to identify the account
    let profile = oauth::fetch_profile(&cred.access_token).await?;

    // Serialize credential for storage in our keyring
    let credential_json = serde_json::to_string(&cred)?;

    Ok(crate::event::OAuthImportData {
        name: profile.email,
        org_id: profile.org_id,
        credential_json,
    })
}

/// Detect which account matches the token currently in Claude Code's keychain.
/// Compares access tokens locally — no API calls.
pub fn spawn_detect_logged_in(app: &AppState, tx: &mpsc::UnboundedSender<Event>) {
    let tx = tx.clone();
    let keyring = app.keyring.clone();
    let oauth_accounts: Vec<String> = app
        .accounts
        .iter()
        .filter(|a| a.config.auth_method == AuthMethod::OAuth)
        .map(|a| a.config.name.clone())
        .collect();

    tokio::spawn(async move {
        let result = tokio::task::spawn_blocking(move || {
            let cc_cred = oauth::read_claude_code_keychain().ok()?;
            for name in &oauth_accounts {
                if let Ok(stored_json) = keyring.get_session_key(name) {
                    if let Ok(stored_cred) =
                        serde_json::from_str::<oauth::OAuthCredential>(&stored_json)
                    {
                        if stored_cred.access_token == cc_cred.access_token {
                            return Some(name.clone());
                        }
                    }
                }
            }
            None
        })
        .await
        .unwrap_or(None);

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

// parse_utilization and parse_resets_at live in oauth.rs (single source of truth)
