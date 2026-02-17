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
        .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)))
        .unwrap_or(0);

    Ok(OAuthCredential {
        access_token: access_token.to_string(),
        refresh_token: refresh_token.to_string(),
        expires_at,
    })
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

/// Get the stored access token. Does NOT refresh — we only use tokens that Claude Code
/// generates to avoid looking like token stripping. If the token is expired, try
/// re-reading from Claude Code's keychain in case it refreshed.
pub fn get_stored_token(
    keyring: &dyn crate::keyring_store::KeyringBackend,
    account_name: &str,
) -> anyhow::Result<String> {
    let cc_credential = read_claude_code_keychain().ok();
    get_stored_token_with_fallback(keyring, account_name, cc_credential)
}

/// Inner function extracted for testability. Accepts the Claude Code credential
/// as a parameter instead of reading it directly from the keychain.
pub(crate) fn get_stored_token_with_fallback(
    keyring: &dyn crate::keyring_store::KeyringBackend,
    account_name: &str,
    cc_credential: Option<OAuthCredential>,
) -> anyhow::Result<String> {
    let stored = keyring
        .get_session_key(account_name)
        .map_err(|e| anyhow::anyhow!("No OAuth credential stored: {e}"))?;

    let cred: OAuthCredential = serde_json::from_str(&stored)
        .map_err(|e| anyhow::anyhow!("Invalid OAuth credential JSON: {e}"))?;

    if !cred.needs_refresh() {
        return Ok(cred.access_token);
    }

    // Token is expired/near-expiry. Try using the Claude Code credential if provided,
    // but ONLY if it belongs to the same account (verified by matching refresh_token).
    // Without this check, account A could silently get account B's credential.
    if let Some(fresh) = cc_credential {
        if !fresh.needs_refresh() && fresh.refresh_token == cred.refresh_token {
            // Same account (matching refresh_token), Claude Code refreshed it
            let json = serde_json::to_string(&fresh)?;
            let _ = keyring.set_session_key(account_name, &json);
            return Ok(fresh.access_token);
        }
    }

    // Return the expired token anyway — the API call will fail with 401
    // and we'll show the cached countdown with an "Expired" status
    Ok(cred.access_token)
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
    use std::collections::HashMap;
    use std::sync::Mutex;

    use crate::error::TrackerError;
    use crate::keyring_store::KeyringBackend;

    struct MockKeyring {
        store: Mutex<HashMap<String, String>>,
    }

    impl MockKeyring {
        fn new() -> Self {
            Self {
                store: Mutex::new(HashMap::new()),
            }
        }

        fn preload(&self, name: &str, value: &str) {
            self.store
                .lock()
                .unwrap()
                .insert(name.into(), value.into());
        }

        fn get_stored(&self, name: &str) -> Option<String> {
            self.store.lock().unwrap().get(name).cloned()
        }
    }

    impl KeyringBackend for MockKeyring {
        fn get_session_key(&self, account_name: &str) -> Result<String, TrackerError> {
            self.store
                .lock()
                .unwrap()
                .get(account_name)
                .cloned()
                .ok_or_else(|| TrackerError::Keyring(format!("not found: {account_name}")))
        }

        fn set_session_key(
            &self,
            account_name: &str,
            session_key: &str,
        ) -> Result<(), TrackerError> {
            self.store
                .lock()
                .unwrap()
                .insert(account_name.into(), session_key.into());
            Ok(())
        }

        fn delete_session_key(&self, account_name: &str) -> Result<(), TrackerError> {
            self.store.lock().unwrap().remove(account_name);
            Ok(())
        }
    }

    // =========================================================================
    // DEFECT: Missing expiresAt defaults to epoch 0, causing needs_refresh()
    // to always return true. Every get_stored_token call for this credential
    // falls through to the Claude Code keychain re-read path.
    // =========================================================================
    #[test]
    fn defect_missing_expires_at_defaults_to_zero_always_needs_refresh() {
        let json = r#"{"claudeAiOauth":{"accessToken":"tok","refreshToken":"ref"}}"#;
        let cred = parse_credential_json(json).unwrap();

        assert_eq!(
            cred.expires_at, 0,
            "Missing expiresAt silently defaults to epoch 0"
        );
        assert!(
            cred.needs_refresh(),
            "DEFECT: credential with expires_at=0 always needs refresh — \
             silent default causes perpetual expiry fallback"
        );
    }

    // =========================================================================
    // FIX VERIFIED: expiresAt as a JSON float is correctly parsed via the
    // as_f64() fallback, instead of silently defaulting to 0.
    // =========================================================================
    #[test]
    fn float_expires_at_correctly_parsed() {
        let json = r#"{"claudeAiOauth":{"accessToken":"tok","refreshToken":"ref","expiresAt":1893456000000.0}}"#;
        let cred = parse_credential_json(json).unwrap();

        assert_eq!(
            cred.expires_at, 1893456000000,
            "Float expiresAt should be correctly parsed as i64"
        );
        assert!(
            !cred.needs_refresh(),
            "Far-future timestamp should not need refresh"
        );
    }

    // =========================================================================
    // FIX VERIFIED: Different account's credential is NOT used as fallback.
    //
    // When Alice's token is expired and Claude Code is logged in as Bob
    // (different refresh_token), Alice gets her own expired token back —
    // Bob's credential is never written to Alice's keyring entry.
    // =========================================================================
    #[test]
    fn different_account_credential_not_used_as_fallback() {
        let mock = MockKeyring::new();

        // Alice's credential is expired (expires_at = 0)
        let alice_cred = OAuthCredential {
            access_token: "alice-token".into(),
            refresh_token: "alice-refresh".into(),
            expires_at: 0,
        };
        mock.preload(
            "alice@example.com",
            &serde_json::to_string(&alice_cred).unwrap(),
        );

        // Claude Code is logged in as Bob (different refresh_token = different account)
        let bob_cred = OAuthCredential {
            access_token: "bob-token".into(),
            refresh_token: "bob-refresh".into(),
            expires_at: i64::MAX,
        };

        // Ask for Alice's token — Bob's credential must NOT be used
        let result =
            get_stored_token_with_fallback(&mock, "alice@example.com", Some(bob_cred)).unwrap();

        // Alice gets her own (expired) token back, not Bob's
        assert_eq!(
            result, "alice-token",
            "Must return Alice's own token, not Bob's"
        );

        // Alice's stored credential was NOT overwritten
        let stored_json = mock.get_stored("alice@example.com").unwrap();
        let stored_cred: OAuthCredential = serde_json::from_str(&stored_json).unwrap();
        assert_eq!(
            stored_cred.refresh_token, "alice-refresh",
            "Alice's credential must not be overwritten with Bob's"
        );
    }

    // =========================================================================
    // FIX VERIFIED: Same account's refreshed credential IS used as fallback.
    //
    // When Claude Code refreshes the token for the same account (matching
    // refresh_token), the fresh credential is correctly picked up and stored.
    // =========================================================================
    #[test]
    fn same_account_refresh_updates_stored_credential() {
        let mock = MockKeyring::new();

        let old_cred = OAuthCredential {
            access_token: "old-token".into(),
            refresh_token: "same-refresh".into(),
            expires_at: 0, // expired
        };
        mock.preload(
            "alice@example.com",
            &serde_json::to_string(&old_cred).unwrap(),
        );

        // Claude Code refreshed the token for the same account (same refresh_token)
        let fresh_cred = OAuthCredential {
            access_token: "new-token".into(),
            refresh_token: "same-refresh".into(),
            expires_at: i64::MAX,
        };

        let result =
            get_stored_token_with_fallback(&mock, "alice@example.com", Some(fresh_cred)).unwrap();

        assert_eq!(result, "new-token", "Should use the refreshed token");

        let stored_json = mock.get_stored("alice@example.com").unwrap();
        let stored_cred: OAuthCredential = serde_json::from_str(&stored_json).unwrap();
        assert_eq!(
            stored_cred.access_token, "new-token",
            "Stored credential should be updated with refreshed token"
        );
    }
}
