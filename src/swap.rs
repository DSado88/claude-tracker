use std::io::Write;

use serde::Serialize;

use crate::config;

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
