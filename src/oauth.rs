use std::time::Duration;

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::app::UsageData;

const USAGE_ENDPOINT: &str = "https://api.anthropic.com/api/oauth/usage";
const PROFILE_ENDPOINT: &str = "https://api.anthropic.com/api/oauth/profile";
const BETA_HEADER: &str = "oauth-2025-04-20";
const USER_AGENT: &str = "claude-code/2.0.32";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthCredential {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: i64, // epoch milliseconds
}

pub struct OAuthProfile {
    pub email: String,
    pub org_id: String,
    pub org_name: String,
}

impl OAuthCredential {
    pub fn needs_refresh(&self) -> bool {
        // Refresh if within 15 minutes of expiry
        let buffer_ms = 15 * 60 * 1000;
        Utc::now().timestamp_millis() + buffer_ms >= self.expires_at
    }
}

/// Read Claude Code's OAuth credentials from macOS Keychain via `security` CLI.
pub fn read_claude_code_keychain() -> anyhow::Result<OAuthCredential> {
    let output = std::process::Command::new("security")
        .args([
            "find-generic-password",
            "-s",
            "Claude Code-credentials",
            "-w",
        ])
        .output()
        .map_err(|e| anyhow::anyhow!("Failed to run security command: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow::anyhow!(
            "No Claude Code credentials found. Log into Claude Code first. ({stderr})"
        ));
    }

    let json_str = String::from_utf8(output.stdout)
        .map_err(|e| anyhow::anyhow!("Invalid UTF-8 in keychain data: {e}"))?;
    let json_str = json_str.trim();

    parse_credential_json(json_str)
}

fn parse_credential_json(json_str: &str) -> anyhow::Result<OAuthCredential> {
    let value: serde_json::Value = serde_json::from_str(json_str)?;

    // Claude Code wraps credentials under "claudeAiOauth"; fall back to top-level
    let creds = value.get("claudeAiOauth").unwrap_or(&value);

    let access_token = creds
        .get("accessToken")
        .or_else(|| creds.get("access_token"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing access_token in Claude Code credentials"))?;

    let refresh_token = creds
        .get("refreshToken")
        .or_else(|| creds.get("refresh_token"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing refresh_token in Claude Code credentials"))?;

    let expires_at = creds
        .get("expiresAt")
        .or_else(|| creds.get("expires_at"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    Ok(OAuthCredential {
        access_token: access_token.to_string(),
        refresh_token: refresh_token.to_string(),
        expires_at,
    })
}

/// Fetch the account profile to identify which account a token belongs to.
pub async fn fetch_profile(access_token: &str) -> anyhow::Result<OAuthProfile> {
    let client = reqwest::Client::new();
    let resp = client
        .get(PROFILE_ENDPOINT)
        .header("Authorization", format!("Bearer {}", access_token))
        .header("anthropic-beta", BETA_HEADER)
        .header("User-Agent", USER_AGENT)
        .timeout(Duration::from_secs(10))
        .send()
        .await?
        .error_for_status()?;

    let json: serde_json::Value = resp.json().await?;

    let account = json
        .get("account")
        .ok_or_else(|| anyhow::anyhow!("Missing account in profile response"))?;
    let org = json
        .get("organization")
        .ok_or_else(|| anyhow::anyhow!("Missing organization in profile response"))?;

    Ok(OAuthProfile {
        email: account
            .get("email")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string(),
        org_id: org
            .get("uuid")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        org_name: org
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string(),
    })
}

/// Fetch usage data using an OAuth access token.
pub async fn fetch_oauth_usage(access_token: &str) -> anyhow::Result<UsageData> {
    let client = reqwest::Client::new();
    let resp = client
        .get(USAGE_ENDPOINT)
        .header("Authorization", format!("Bearer {}", access_token))
        .header("anthropic-beta", BETA_HEADER)
        .header("User-Agent", USER_AGENT)
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

/// Get the stored access token. Does NOT refresh — we only use tokens that Claude Code
/// generates to avoid looking like token stripping. If the token is expired, try
/// re-reading from Claude Code's keychain in case it refreshed.
pub fn get_stored_token(
    keyring: &dyn crate::keyring_store::KeyringBackend,
    account_name: &str,
) -> anyhow::Result<String> {
    let stored = keyring
        .get_session_key(account_name)
        .map_err(|e| anyhow::anyhow!("No OAuth credential stored: {e}"))?;

    let cred: OAuthCredential = serde_json::from_str(&stored)
        .map_err(|e| anyhow::anyhow!("Invalid OAuth credential JSON: {e}"))?;

    if !cred.needs_refresh() {
        return Ok(cred.access_token);
    }

    // Token is expired/near-expiry. Try re-reading from Claude Code's keychain
    // in case Claude Code refreshed it.
    if let Ok(fresh) = read_claude_code_keychain() {
        if !fresh.needs_refresh() {
            // Claude Code refreshed it — update our stored copy
            let json = serde_json::to_string(&fresh)?;
            let _ = keyring.set_session_key(account_name, &json);
            return Ok(fresh.access_token);
        }
    }

    // Return the expired token anyway — the API call will fail with 401
    // and we'll show the cached countdown with an "Expired" status
    Ok(cred.access_token)
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

fn parse_resets_at(bucket: &serde_json::Value) -> Option<chrono::DateTime<Utc>> {
    bucket
        .get("resets_at")
        .and_then(|v| v.as_str())
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc))
}
