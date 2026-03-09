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

1. **Log in directly** — press `L` to open a browser and authenticate with any Anthropic account
2. **Monitor all accounts simultaneously** — usage percentages update every 3 minutes
3. **Countdown timers tick locally** — once fetched, reset times are accurate without re-polling
4. **Auto-clear on reset** — when a countdown expires, usage drops to 0% immediately (no poll needed)
5. **Auto-refresh tokens** — expired access tokens are refreshed automatically using stored refresh tokens
6. **Logged In detection** — shows which account matches Claude Code's current keychain token
7. **Mark active account** — cosmetic marker for which account you intend to use

## Adding Accounts

### OAuth Login (recommended)

Press `L` in the tracker. A fresh Chrome window opens — log into your Anthropic account, and the tracker captures its own token pair (access + refresh). Repeat for each account.

Each `L` opens an isolated browser profile so you can log into different accounts without cookie conflicts.

### Import from Claude Code

Press `i` to import the account currently logged into Claude Code. This reads Claude Code's keychain entry and identifies the account. Useful if you're already authenticated.

### Manual (Session Key)

Press `a` to add an account manually:

1. Open [claude.ai](https://claude.ai) in your browser
2. Open DevTools → Application → Cookies → `sessionKey`
3. Get your org ID: open the browser console and run `fetch('/api/organizations').then(r => r.json()).then(d => console.log(d))`
4. Enter the name, session key, and org ID

Note: Session keys expire when you log out of the browser.

## Token Handling

The tracker stores full OAuth credentials (access token + refresh token + expiry). When an access token expires, it automatically refreshes using the stored refresh token — no manual re-import needed.

Tokens obtained via `L` are independent of Claude Code's tokens, so Claude Code's own token refreshes won't invalidate the tracker's tokens.

**What's stored where:**

| Data | Location | Notes |
|------|----------|-------|
| Account names + org IDs | `~/.config/claude-tracker/config.toml` | No secrets |
| OAuth credentials | macOS Keychain under `claude-tracker` service | Per-account, includes refresh token |
| Claude Code's own credentials | macOS Keychain under `Claude Code-credentials` service | Read-only (used by `i` import) |

**Nothing is stored in plaintext on disk.** All tokens live in the macOS Keychain.

## Install

```bash
git clone https://github.com/DSado88/claude-tracker.git
cd claude-tracker
cargo build --release
```

Binary will be at `target/release/claude-tracker`.

## Status Column

| Status | Meaning |
|--------|---------|
| **Logged In** | This account's token matches Claude Code's current keychain entry |
| **Live** | Data fetched within the last 2 minutes |
| **5m ago** | Data is stale (last fetched 5 minutes ago) |
| **Expired — re-import (i)** | Token expired and refresh failed |
| **Rate limited — try later** | API rate limit hit (429) |
| **Timeout** | Request timed out |
| **No network** | DNS or connection failure |
| **--** | No data fetched yet |

## Keybindings

| Key | Action |
|-----|--------|
| `j` / `k` | Navigate up/down |
| `r` | Refresh all accounts |
| `R` | Refresh selected account |
| `L` | OAuth login (opens browser, adds account) |
| `i` | Import from Claude Code keychain |
| `s` / `Enter` | Mark selected account as active (cosmetic) |
| `a` | Add account manually (session key + org ID) |
| `e` | Edit account |
| `d` / `x` | Delete account |
| `?` | Help |
| `q` / `Ctrl+C` | Quit |

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
- Google Chrome (for OAuth login flow)
