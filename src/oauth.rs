use std::time::Duration;

use chrono::Utc;

use crate::app::UsageData;

const USAGE_ENDPOINT: &str = "https://api.anthropic.com/api/oauth/usage";
const PROFILE_ENDPOINT: &str = "https://api.anthropic.com/api/oauth/profile";
const BETA_HEADER: &str = "oauth-2025-04-20";
const USER_AGENT: &str = "claude-code/2.0.32";

pub struct OAuthProfile {
    pub email: String,
    pub org_id: String,
}

/// Read Claude Code's access token from macOS Keychain via `security` CLI.
pub fn read_claude_code_access_token() -> anyhow::Result<String> {
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

    parse_access_token(json_str)
}

fn parse_access_token(json_str: &str) -> anyhow::Result<String> {
    let value: serde_json::Value = serde_json::from_str(json_str)?;

    // Claude Code wraps credentials under "claudeAiOauth"; fall back to top-level
    let creds = value.get("claudeAiOauth").unwrap_or(&value);

    creds
        .get("accessToken")
        .or_else(|| creds.get("access_token"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow::anyhow!("Missing access_token in Claude Code credentials"))
}

/// Fetch the account profile to identify which account a token belongs to.
pub async fn fetch_profile(access_token: &str) -> anyhow::Result<OAuthProfile> {
    let client = crate::api::http_client();
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
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow::anyhow!("Missing or empty org ID in profile response"))?
            .to_string(),
    })
}

/// Fetch usage data using an OAuth access token.
pub async fn fetch_oauth_usage(access_token: &str) -> anyhow::Result<UsageData> {
    let client = crate::api::http_client();
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

/// Extract the access token from a stored keyring value.
/// Handles both the old JSON format ({"access_token":"...","refresh_token":"...",...})
/// and the new plain-string format.
pub(crate) fn normalize_stored_token(raw: &str) -> String {
    if raw.starts_with('{') {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) {
            if let Some(token) = value
                .get("access_token")
                .or_else(|| value.get("accessToken"))
                .and_then(|v| v.as_str())
            {
                return token.to_string();
            }
        }
    }
    raw.to_string()
}

pub(crate) fn parse_utilization(bucket: &serde_json::Value) -> u32 {
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

pub(crate) fn parse_resets_at(bucket: &serde_json::Value) -> Option<chrono::DateTime<Utc>> {
    bucket
        .get("resets_at")
        .and_then(|v| v.as_str())
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc))
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_access_token_wrapped_format() {
        let json = r#"{"claudeAiOauth":{"accessToken":"tok123","refreshToken":"ref","expiresAt":0}}"#;
        let token = parse_access_token(json).unwrap();
        assert_eq!(token, "tok123");
    }

    #[test]
    fn parse_access_token_top_level_format() {
        let json = r#"{"access_token":"tok456"}"#;
        let token = parse_access_token(json).unwrap();
        assert_eq!(token, "tok456");
    }

    #[test]
    fn parse_access_token_missing_returns_error() {
        let json = r#"{"claudeAiOauth":{"refreshToken":"ref"}}"#;
        assert!(parse_access_token(json).is_err());
    }

    #[test]
    fn normalize_old_json_format() {
        let old = r#"{"access_token":"eyJtoken","refresh_token":"rt-xxx","expires_at":0}"#;
        assert_eq!(normalize_stored_token(old), "eyJtoken");
    }

    #[test]
    fn normalize_plain_token_passthrough() {
        assert_eq!(normalize_stored_token("eyJplaintoken"), "eyJplaintoken");
    }
}
