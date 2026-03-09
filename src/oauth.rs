use std::time::Duration;

use chrono::Utc;

use crate::app::UsageData;

const USAGE_ENDPOINT: &str = "https://api.anthropic.com/api/oauth/usage";
const PROFILE_ENDPOINT: &str = "https://api.anthropic.com/api/oauth/profile";
const REFRESH_ENDPOINT: &str = "https://api.anthropic.com/v1/oauth/token";
pub const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const BETA_HEADER: &str = "oauth-2025-04-20";
const USER_AGENT: &str = "claude-code/2.0.32";

pub struct OAuthProfile {
    pub email: String,
    pub org_id: String,
}

/// Read Claude Code's access token from the default macOS Keychain entry.
pub fn read_claude_code_access_token() -> anyhow::Result<String> {
    read_keychain_token("Claude Code-credentials")
}

/// Read all Claude Code raw credentials from macOS Keychain.
///
/// Claude Code uses per-config-directory keychain entries:
/// - Default: `"Claude Code-credentials"`
/// - Alternate: `"Claude Code-credentials-{hash}"` where hash = first 8 chars of sha256(config_dir)
///
/// Returns deduplicated raw credential JSON strings (preserving refresh tokens).
pub fn read_all_claude_code_credentials() -> anyhow::Result<Vec<String>> {
    let service_names = discover_credential_services()?;
    if service_names.is_empty() {
        return Err(anyhow::anyhow!(
            "No Claude Code credentials found. Log into Claude Code first."
        ));
    }

    let mut credentials = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for svc in &service_names {
        if let Ok(raw) = read_keychain_raw(svc) {
            // Dedup by access token, but keep full raw credential
            if let Ok(access) = parse_access_token(&raw) {
                if seen.insert(access) {
                    credentials.push(raw);
                }
            }
        }
    }

    if credentials.is_empty() {
        return Err(anyhow::anyhow!(
            "Found {} keychain entries but none contained a valid token.",
            service_names.len()
        ));
    }
    Ok(credentials)
}

/// Discover all `Claude Code-credentials*` service names in the login keychain.
fn discover_credential_services() -> anyhow::Result<Vec<String>> {
    let output = std::process::Command::new("security")
        .args(["dump-keychain"])
        .output()
        .map_err(|e| anyhow::anyhow!("Failed to run security dump-keychain: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut services = Vec::new();
    for line in stdout.lines() {
        // Lines look like: "svce"<blob>="Claude Code-credentials"
        if let Some(rest) = line.strip_prefix("    \"svce\"<blob>=\"") {
            if let Some(name) = rest.strip_suffix('"') {
                if name.starts_with("Claude Code-credentials") {
                    services.push(name.to_string());
                }
            }
        }
    }
    services.dedup();
    Ok(services)
}

/// Read a single access token from a specific keychain service name.
fn read_keychain_token(service: &str) -> anyhow::Result<String> {
    let raw = read_keychain_raw(service)?;
    parse_access_token(&raw)
}

/// Read the raw credential string from a keychain service (preserving all fields).
fn read_keychain_raw(service: &str) -> anyhow::Result<String> {
    let output = std::process::Command::new("security")
        .args(["find-generic-password", "-s", service, "-w"])
        .output()
        .map_err(|e| anyhow::anyhow!("Failed to run security command: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow::anyhow!(
            "No credentials found for service '{service}'. ({stderr})"
        ));
    }

    let raw = String::from_utf8(output.stdout)
        .map_err(|e| anyhow::anyhow!("Invalid UTF-8 in keychain data: {e}"))?;
    Ok(raw.trim().to_string())
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
        .await?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        eprintln!(
            "[oauth/profile] HTTP {status} | body: {}",
            &body[..body.len().min(500)],
        );
        return Err(anyhow::anyhow!("HTTP {} {}", status.as_u16(), status.canonical_reason().unwrap_or("")));
    }

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
        .await?;

    let status = resp.status();
    if !status.is_success() {
        let retry_after = resp
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let body = resp.text().await.unwrap_or_default();
        eprintln!(
            "[oauth/usage] HTTP {status} | retry-after: {} | body: {}",
            retry_after.as_deref().unwrap_or("none"),
            &body[..body.len().min(500)],
        );
        return Err(anyhow::anyhow!(
            "HTTP {} {}{}",
            status.as_u16(),
            status.canonical_reason().unwrap_or(""),
            retry_after
                .map(|r| format!(" (retry-after: {r})"))
                .unwrap_or_default()
        ));
    }

    let raw_body = resp.text().await?;
    eprintln!("[oauth/usage] raw response: {}", &raw_body[..raw_body.len().min(1000)]);
    let body: serde_json::Value = serde_json::from_str(&raw_body)?;

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

/// Extract the refresh token from a stored credential (JSON formats only).
/// Returns None for plain access token strings (no refresh token available).
pub(crate) fn extract_refresh_token(raw: &str) -> Option<String> {
    if !raw.starts_with('{') {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(raw).ok()?;
    let creds = value.get("claudeAiOauth").unwrap_or(&value);
    creds
        .get("refreshToken")
        .or_else(|| creds.get("refresh_token"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// Refresh an expired access token using the refresh token.
pub async fn refresh_access_token(refresh_token: &str) -> anyhow::Result<RefreshResponse> {
    let client = crate::api::http_client();
    let resp = client
        .post(REFRESH_ENDPOINT)
        .header("User-Agent", USER_AGENT)
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", CLIENT_ID),
        ])
        .timeout(Duration::from_secs(10))
        .send()
        .await?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        eprintln!(
            "[oauth/refresh] HTTP {status} | body: {}",
            &body[..body.len().min(500)],
        );
        return Err(anyhow::anyhow!("Refresh failed: HTTP {}", status.as_u16()));
    }

    let json: serde_json::Value = resp.json().await?;
    let access_token = json
        .get("access_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing access_token in refresh response"))?
        .to_string();
    let new_refresh = json
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let expires_in = json
        .get("expires_in")
        .and_then(|v| v.as_i64())
        .unwrap_or(28800); // default 8 hours

    let expires_at = Utc::now().timestamp_millis() + (expires_in * 1000);

    Ok(RefreshResponse {
        access_token,
        refresh_token: new_refresh,
        expires_at,
    })
}

pub struct RefreshResponse {
    pub access_token: String,
    /// New refresh token, if the server rotated it. Otherwise reuse the old one.
    pub refresh_token: Option<String>,
    /// Epoch millis when the new access token expires.
    pub expires_at: i64,
}

/// Rebuild the stored credential JSON with updated tokens.
pub(crate) fn update_credential_json(
    raw: &str,
    new_access: &str,
    new_refresh: Option<&str>,
    expires_at: i64,
) -> String {
    if let Ok(mut value) = serde_json::from_str::<serde_json::Value>(raw) {
        // Update in the claudeAiOauth wrapper if present, otherwise top-level
        let creds = if value.get("claudeAiOauth").is_some() {
            value.get_mut("claudeAiOauth").unwrap()
        } else {
            &mut value
        };

        if let Some(obj) = creds.as_object_mut() {
            // Update access token (try both field names)
            if obj.contains_key("accessToken") {
                obj.insert("accessToken".into(), new_access.into());
            } else {
                obj.insert("access_token".into(), new_access.into());
            }
            // Update refresh token if rotated
            if let Some(rt) = new_refresh {
                if obj.contains_key("refreshToken") {
                    obj.insert("refreshToken".into(), rt.into());
                } else {
                    obj.insert("refresh_token".into(), rt.into());
                }
            }
            // Update expiry
            if obj.contains_key("expiresAt") {
                obj.insert("expiresAt".into(), expires_at.into());
            } else {
                obj.insert("expires_at".into(), expires_at.into());
            }
        }
        serde_json::to_string(&value).unwrap_or_else(|_| raw.to_string())
    } else {
        // Plain token — can't store refresh info, just return new access token
        new_access.to_string()
    }
}

/// Extract the access token from a stored keyring value.
/// Handles both the old JSON format ({"access_token":"...","refresh_token":"...",...})
/// and the new plain-string format.
pub(crate) fn normalize_stored_token(raw: &str) -> String {
    if raw.starts_with('{') {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) {
            let creds = value.get("claudeAiOauth").unwrap_or(&value);
            if let Some(token) = creds
                .get("accessToken")
                .or_else(|| creds.get("access_token"))
                .and_then(|v| v.as_str())
            {
                return token.to_string();
            }
        }
    }
    raw.to_string()
}

/// Generate a random state string for CSRF protection in OAuth flows.
pub fn generate_random_state() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let bytes: Vec<u8> = (0..32).map(|_| rng.gen()).collect();
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub(crate) fn parse_utilization(bucket: &serde_json::Value) -> u32 {
    bucket
        .get("utilization")
        .and_then(|v| v.as_u64().map(|n| n as f64).or_else(|| v.as_f64()))
        .map(|v| v.round() as u32)
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

    #[test]
    fn normalize_wrapped_format() {
        let raw = r#"{"claudeAiOauth":{"accessToken":"tok-wrapped","refreshToken":"rt","expiresAt":0}}"#;
        assert_eq!(normalize_stored_token(raw), "tok-wrapped");
    }

    #[test]
    fn extract_refresh_token_wrapped() {
        let raw = r#"{"claudeAiOauth":{"accessToken":"at","refreshToken":"rt-123","expiresAt":0}}"#;
        assert_eq!(extract_refresh_token(raw), Some("rt-123".to_string()));
    }

    #[test]
    fn extract_refresh_token_old_format() {
        let raw = r#"{"access_token":"at","refresh_token":"rt-old","expires_at":0}"#;
        assert_eq!(extract_refresh_token(raw), Some("rt-old".to_string()));
    }

    #[test]
    fn extract_refresh_token_plain_string_returns_none() {
        assert_eq!(extract_refresh_token("eyJplaintoken"), None);
    }

    #[test]
    fn update_credential_json_wrapped() {
        let raw = r#"{"claudeAiOauth":{"accessToken":"old-at","refreshToken":"old-rt","expiresAt":0}}"#;
        let updated = update_credential_json(raw, "new-at", Some("new-rt"), 999);
        let v: serde_json::Value = serde_json::from_str(&updated).unwrap();
        let creds = v.get("claudeAiOauth").unwrap();
        assert_eq!(creds["accessToken"], "new-at");
        assert_eq!(creds["refreshToken"], "new-rt");
        assert_eq!(creds["expiresAt"], 999);
    }

    #[test]
    fn update_credential_json_no_refresh_rotation() {
        let raw = r#"{"claudeAiOauth":{"accessToken":"old","refreshToken":"keep-me","expiresAt":0}}"#;
        let updated = update_credential_json(raw, "new-at", None, 500);
        let v: serde_json::Value = serde_json::from_str(&updated).unwrap();
        let creds = v.get("claudeAiOauth").unwrap();
        assert_eq!(creds["accessToken"], "new-at");
        assert_eq!(creds["refreshToken"], "keep-me", "refresh token should be unchanged");
    }
}
