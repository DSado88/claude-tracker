# Claude Tracker

Terminal dashboard for monitoring Claude AI usage across multiple accounts. See all your accounts side by side — utilization %, progress bars, and countdown timers to reset.

```
 Claude Tracker                          Last refresh: 12s ago
 #  | Name                | 5h % | 5h Bar     | 5h Reset | 7d % | 7d Bar     | 7d Reset | Status
 >1 | user@personal.com * |  42% | ████░░░░░░ | 2h 14m   |  18% | ██░░░░░░░░ | 6d 23h   | Logged In
  2 | user@work.com       |  87% | █████████░ |   38m    |  65% | ███████░░░ | 4d 11h   | Live
  3 | user@client.com     | 100% | ██████████ |   12m    |  91% | █████████░ | 2d 06h   | Live
```

Built with Rust + [ratatui](https://github.com/ratatui/ratatui). Designed for people who rotate between multiple Claude accounts to maximize availability.

## How It Works

1. **Import accounts** from Claude Code's OAuth tokens (stored in macOS Keychain)
2. **Monitor all accounts simultaneously** — usage percentages update every 3 minutes
3. **Countdown timers tick locally** — once fetched, reset times are accurate without re-polling
4. **Logged In detection** — shows which account matches Claude Code's current keychain token
5. **Mark active account** — cosmetic marker for which account you intend to use

## Security & Token Handling

**We never refresh tokens ourselves.** All OAuth tokens come directly from Claude Code's own keychain entry. We only read them to:

- Call the usage API (read-only, `GET /api/oauth/usage`)
- Call the profile API once at import (to identify the account)
- Compare access tokens locally to detect which account is currently logged in

This avoids any appearance of token stripping. If an access token expires, the tracker tries re-reading Claude Code's keychain (in case Claude Code refreshed it), but **only if the refresh token matches** — preventing one account's credential from silently overwriting another's. If still expired, the cached countdown remains accurate — you just won't get updated utilization % until you use Claude Code with that account again.

**What's stored where:**

| Data | Location | Notes |
|------|----------|-------|
| Account names + org IDs | `~/.config/claude-tracker/config.toml` | No secrets |
| OAuth credentials (access + refresh tokens) | macOS Keychain under `claude-tracker` service | Per-account entries |
| Claude Code's own credentials | macOS Keychain under `Claude Code-credentials` service | Read-only (we read from here on import) |

**Nothing is stored in plaintext on disk.** All tokens live in the macOS Keychain.

## Install

```bash
git clone https://github.com/DSado88/claude-tracker.git
cd claude-tracker
cargo build --release
```

Binary will be at `target/release/claude-tracker`.

## Setup — Importing Accounts

You import each account once by logging into it in Claude Code, then pressing `i` in the tracker.

### First account:

```bash
# Make sure you're logged into Claude Code with account #1
claude  # (use it normally, or just run any command to ensure token is fresh)

# In another terminal, launch the tracker
./target/release/claude-tracker

# Press 'i' to import — it reads the keychain and identifies the account
```

### Additional accounts:

```bash
# Log into account #2 using a separate Claude Code config directory
CLAUDE_CONFIG_DIR=~/.claude-acct2 claude
# Type /login, authenticate with account #2 in the browser

# Back in the tracker, press 'i' again — imports account #2
# Repeat for account #3 with CLAUDE_CONFIG_DIR=~/.claude-acct3
```

After importing, all accounts poll independently. The refresh tokens persist in your keychain — you don't need to re-import unless a token gets revoked.

## Status Column

| Status | Meaning |
|--------|---------|
| **Logged In** | This account's token matches Claude Code's current keychain entry |
| **Live** | Data fetched within the last 2 minutes |
| **5m ago** | Data is stale (last fetched 5 minutes ago) |
| **--** | No data fetched yet |

"Logged In" tells you which account Claude Code will use for new sessions. Note that existing Claude Code windows keep their token in memory — they'll keep using whatever account they were started with until you `/login` again in that window.

## Keybindings

| Key | Action |
|-----|--------|
| `j` / `k` | Navigate up/down |
| `r` | Refresh all accounts |
| `R` | Refresh selected account |
| `i` | Import from Claude Code (reads current keychain) |
| `s` / `Enter` | Mark selected account as active (cosmetic) |
| `a` | Add account manually (session key + org ID) |
| `e` | Edit account |
| `d` / `x` | Delete account |
| `?` | Help |
| `q` / `Ctrl+C` | Quit |

## Manual Account Setup (Session Key)

If you prefer not to use OAuth import, you can add accounts manually with `a`:

1. Open [claude.ai](https://claude.ai) in your browser
2. Open DevTools → Application → Cookies → `sessionKey`
3. Get your org ID: open the browser console and run `fetch('/api/organizations').then(r => r.json()).then(d => console.log(d))`
4. In the tracker, press `a` and enter the name, session key, and org ID

Note: Session keys expire when you log out of the browser. OAuth tokens (via `i` import) are more durable.

## Config

`~/.config/claude-tracker/config.toml`:

```toml
[settings]
poll_interval_secs = 180  # minimum 30, clamped on load
active_account = 0

[[accounts]]
name = "user@example.com"
org_id = "65f10de7-..."
auth_method = "oauth"
```

Config writes are atomic (temp file + rename) to prevent corruption if the app crashes mid-write.

## Dependencies

- macOS (uses Keychain for credential storage)
- Rust 2021 edition
- Claude Code installed and logged in (for OAuth import)
