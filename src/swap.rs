use std::io::Write;

use serde::Serialize;

use crate::config::{self, AuthMethod};
use crate::keyring_store::KeyringBackend;
use crate::oauth::OAuthCredential;

#[derive(Serialize)]
struct ActiveSession {
    session_key: String,
    org_id: String,
}

pub fn write_active_session(session_key: &str, org_id: &str) -> anyhow::Result<()> {
    let path = config::active_session_path()?;
    let dir = path.parent().unwrap();
    std::fs::create_dir_all(dir)?;

    // Atomic write: temp file + rename
    let tmp_path = dir.join(".active_session.tmp");
    let session = ActiveSession {
        session_key: session_key.to_string(),
        org_id: org_id.to_string(),
    };
    let json = serde_json::to_string_pretty(&session)?;

    let mut file = std::fs::File::create(&tmp_path)?;
    file.write_all(json.as_bytes())?;
    file.sync_all()?;
    std::fs::rename(&tmp_path, &path)?;

    Ok(())
}

/// Write an OAuth credential into Claude Code's keychain entry so Claude Code
/// picks it up on its next API call — no /login needed.
pub fn swap_claude_code_credential(
    keyring: &dyn KeyringBackend,
    account_name: &str,
    auth_method: &AuthMethod,
) -> anyhow::Result<()> {
    match auth_method {
        AuthMethod::OAuth => {
            // Read our stored OAuth credential JSON
            let stored = keyring
                .get_session_key(account_name)
                .map_err(|e| anyhow::anyhow!("No OAuth credential for '{}': {}", account_name, e))?;

            let cred: OAuthCredential = serde_json::from_str(&stored)
                .map_err(|e| anyhow::anyhow!("Invalid OAuth credential: {}", e))?;

            // Build the JSON in Claude Code's expected format
            let cc_json = serde_json::json!({
                "claudeAiOauth": {
                    "accessToken": cred.access_token,
                    "refreshToken": cred.refresh_token,
                    "expiresAt": cred.expires_at
                }
            });
            let cc_str = serde_json::to_string(&cc_json)?;

            // Write to Claude Code's keychain entry via `security` CLI
            write_claude_code_keychain(&cc_str)?;

            Ok(())
        }
        AuthMethod::SessionKey => {
            // For session key accounts, write active_session.json (legacy path)
            let session_key = keyring
                .get_session_key(account_name)
                .map_err(|e| anyhow::anyhow!("No session key for '{}': {}", account_name, e))?;
            // org_id would need to be passed separately for this path
            // For now, just update the keychain — session key swap is the old path
            Err(anyhow::anyhow!(
                "Session key accounts use 's' swap with active_session.json"
            ))
        }
    }
}

/// Overwrite Claude Code's keychain entry with new credential JSON.
fn write_claude_code_keychain(json_str: &str) -> anyhow::Result<()> {
    // First, delete the existing entry (security doesn't have an "update" command)
    let _ = std::process::Command::new("security")
        .args(["delete-generic-password", "-s", "Claude Code-credentials"])
        .output();

    // Get the macOS username for the account field
    let username = std::env::var("USER").unwrap_or_else(|_| "unknown".to_string());

    // Add the new entry
    let output = std::process::Command::new("security")
        .args([
            "add-generic-password",
            "-s", "Claude Code-credentials",
            "-a", &username,
            "-w", json_str,
            "-U", // update if exists
        ])
        .output()
        .map_err(|e| anyhow::anyhow!("Failed to run security command: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow::anyhow!("Failed to write Claude Code keychain: {stderr}"));
    }

    Ok(())
}
