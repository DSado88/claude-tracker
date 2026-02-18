use std::sync::Arc;

use chrono::{DateTime, Utc};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tokio::sync::mpsc;

use crate::config::{self, AccountConfig, AuthMethod, Config};
use crate::event::{Event, OAuthImportData};
use crate::keyring_store::KeyringBackend;

#[derive(Debug, Clone)]
pub struct UsageData {
    pub utilization: u32,
    pub resets_at: Option<DateTime<Utc>>,
    pub weekly_utilization: Option<u32>,
    pub weekly_resets_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AccountStatus {
    Idle,
    Ok,
    Error(String),
}

#[derive(Debug, Clone)]
pub struct AccountState {
    pub config: AccountConfig,
    pub usage: Option<UsageData>,
    pub status: AccountStatus,
    pub last_fetched: Option<DateTime<Utc>>,
    /// Cached token loaded from keyring at startup/import — avoids keychain prompts on every poll.
    pub cached_token: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AppMode {
    Normal,
    AddAccount,
    EditAccount(usize),
    ConfirmDelete,
    ConfirmSwap,
    Help,
}

#[derive(Debug, Default)]
pub struct InputFields {
    pub name: String,
    pub session_key: String,
    pub org_id: String,
    pub focused_field: usize,
}

impl InputFields {
    pub fn clear(&mut self) {
        self.name.clear();
        self.session_key.clear();
        self.org_id.clear();
        self.focused_field = 0;
    }

    pub fn current_field_mut(&mut self) -> &mut String {
        match self.focused_field {
            0 => &mut self.name,
            1 => &mut self.session_key,
            2 => &mut self.org_id,
            _ => unreachable!("focused_field must be 0..2, got {}", self.focused_field),
        }
    }

    pub fn next_field(&mut self) {
        self.focused_field = (self.focused_field + 1) % 3;
    }

    pub fn prev_field(&mut self) {
        self.focused_field = if self.focused_field == 0 {
            2
        } else {
            self.focused_field - 1
        };
    }
}

pub struct AppState {
    pub accounts: Vec<AccountState>,
    pub selected_index: usize,
    pub active_account_index: usize,
    pub mode: AppMode,
    pub should_quit: bool,
    pub last_poll: Option<DateTime<Utc>>,
    pub status_message: Option<(String, DateTime<Utc>)>,
    pub input_fields: InputFields,
    pub poll_interval_secs: u64,
    pub keyring: Arc<dyn KeyringBackend>,
    /// Which account name matches the token currently in Claude Code's keychain.
    pub logged_in_account: Option<String>,
}

impl AppState {
    pub fn from_config(config: Config, keyring: Arc<dyn KeyringBackend>) -> Self {
        let accounts: Vec<AccountState> = config
            .accounts
            .iter()
            .map(|ac| {
                let cached_token = keyring.get_session_key(&ac.name).ok();
                AccountState {
                    config: ac.clone(),
                    usage: None,
                    status: AccountStatus::Idle,
                    last_fetched: None,
                    cached_token,
                }
            })
            .collect();

        let active = config.settings.active_account.min(accounts.len().saturating_sub(1));

        Self {
            accounts,
            selected_index: 0,
            active_account_index: active,
            mode: AppMode::Normal,
            should_quit: false,
            last_poll: None,
            status_message: None,
            input_fields: InputFields::default(),
            logged_in_account: None,
            poll_interval_secs: config.settings.poll_interval_secs,
            keyring,
        }
    }

    /// Apply a usage result by account name (not index) so deletions can't misroute results.
    pub fn apply_usage_result(
        &mut self,
        account_name: &str,
        result: Result<UsageData, String>,
    ) {
        if let Some(account) = self
            .accounts
            .iter_mut()
            .find(|a| a.config.name == account_name)
        {
            match result {
                Ok(data) => {
                    account.usage = Some(data);
                    account.status = AccountStatus::Ok;
                    account.last_fetched = Some(Utc::now());
                }
                Err(msg) => {
                    account.status = AccountStatus::Error(msg);
                }
            }
            self.last_poll = Some(Utc::now());
        }
        // If account was deleted while fetch was in flight, result and last_poll are
        // both silently discarded — no misleading "Last refresh" in the status bar.
    }

    pub fn clear_stale_messages(&mut self) {
        if let Some((_, time)) = &self.status_message {
            if Utc::now().signed_duration_since(*time).num_seconds() > 5 {
                self.status_message = None;
            }
        }
    }

    pub fn set_status(&mut self, msg: String) {
        self.status_message = Some((msg, Utc::now()));
    }

    fn save_config(&mut self) {
        let cfg = Config {
            settings: config::Settings {
                poll_interval_secs: self.poll_interval_secs,
                active_account: self.active_account_index,
            },
            accounts: self.accounts.iter().map(|a| a.config.clone()).collect(),
        };
        if let Err(e) = config::save(&cfg) {
            self.set_status(format!("Failed to save config: {e}"));
        }
    }

    /// Returns Some(index) on success, None on failure.
    fn add_account(&mut self, name: String, session_key: String, org_id: String) -> Option<usize> {
        if self.accounts.iter().any(|a| a.config.name == name) {
            self.set_status(format!("Account '{}' already exists", name));
            return None;
        }

        if let Err(e) = self.keyring.set_session_key(&name, &session_key) {
            self.set_status(format!("Keyring error: {e}"));
            return None;
        }

        let ac = AccountConfig {
            name,
            org_id,
            auth_method: AuthMethod::SessionKey,
        };
        self.accounts.push(AccountState {
            config: ac,
            usage: None,
            status: AccountStatus::Idle,
            last_fetched: None,
            cached_token: Some(session_key),
        });
        self.save_config();
        self.set_status("Account added".to_string());
        Some(self.accounts.len() - 1)
    }

    /// Write new key first. Only delete old key after new key write succeeds.
    fn update_account(&mut self, index: usize, name: String, session_key: String, org_id: String) {
        // Immutable borrow to read old name — released before keyring ops
        let Some(old_name) = self.accounts.get(index).map(|a| a.config.name.clone()) else {
            return;
        };
        let name_changed = old_name != name;

        // Write new key FIRST -- if this fails, old key is preserved
        if let Err(e) = self.keyring.set_session_key(&name, &session_key) {
            self.set_status(format!("Keyring error: {e}"));
            return;
        }

        // Only delete old key AFTER new key is safely stored
        if name_changed {
            if let Err(e) = self.keyring.delete_session_key(&old_name) {
                self.set_status(format!("Warning: old key not deleted: {e}"));
            }
        }

        // Now mutate the account
        if let Some(account) = self.accounts.get_mut(index) {
            account.config.name = name;
            account.config.org_id = org_id;
            account.cached_token = Some(session_key);
            account.usage = None;
            account.status = AccountStatus::Idle;
        }
        self.save_config();
        self.set_status("Account updated".to_string());
    }

    fn delete_selected(&mut self) {
        if self.selected_index < self.accounts.len() {
            let name = self.accounts[self.selected_index].config.name.clone();
            if let Err(e) = self.keyring.delete_session_key(&name) {
                self.set_status(format!("Warning: key not deleted from keyring: {e}"));
            }
            self.accounts.remove(self.selected_index);

            if self.accounts.is_empty() {
                self.selected_index = 0;
                self.active_account_index = 0;
            } else {
                if self.selected_index >= self.accounts.len() {
                    self.selected_index = self.accounts.len() - 1;
                }
                if self.active_account_index >= self.accounts.len() {
                    self.active_account_index = self.accounts.len() - 1;
                }
            }
            self.save_config();
            self.set_status("Account deleted".to_string());
        }
    }

    /// Import an OAuth account from Claude Code. If an account with the same name
    /// already exists, update its credentials. Otherwise, add a new account.
    /// Returns the account index on success.
    pub fn import_oauth_account(&mut self, data: OAuthImportData) -> Option<usize> {
        // Store just the access token in our keyring
        if let Err(e) = self.keyring.set_session_key(&data.name, &data.access_token) {
            self.set_status(format!("Keyring error: {e}"));
            return None;
        }

        // Check if account already exists (by name)
        if let Some(pos) = self.accounts.iter().position(|a| a.config.name == data.name) {
            self.accounts[pos].config.org_id = data.org_id;
            self.accounts[pos].config.auth_method = AuthMethod::OAuth;
            self.accounts[pos].cached_token = Some(data.access_token);
            self.accounts[pos].usage = None;
            self.accounts[pos].status = AccountStatus::Idle;
            self.save_config();
            self.set_status(format!("Updated OAuth account '{}'", data.name));
            return Some(pos);
        }

        // Add new account
        let ac = AccountConfig {
            name: data.name.clone(),
            org_id: data.org_id,
            auth_method: AuthMethod::OAuth,
        };
        self.accounts.push(AccountState {
            config: ac,
            usage: None,
            status: AccountStatus::Idle,
            last_fetched: None,
            cached_token: Some(data.access_token),
        });
        self.save_config();
        self.set_status(format!("Imported OAuth account '{}'", data.name));
        Some(self.accounts.len() - 1)
    }

    fn swap_to_selected(&mut self) {
        if self.selected_index < self.accounts.len() {
            let name = self.accounts[self.selected_index].config.name.clone();
            self.active_account_index = self.selected_index;
            self.save_config();
            self.set_status(format!("Active: '{name}'"));
        }
    }
}

pub fn handle_key(app: &mut AppState, key: KeyEvent, tx: &mpsc::UnboundedSender<Event>) {
    match &app.mode {
        AppMode::Normal => handle_normal_key(app, key, tx),
        AppMode::AddAccount => handle_input_key(app, key, tx),
        AppMode::EditAccount(_) => handle_input_key(app, key, tx),
        AppMode::ConfirmDelete => handle_confirm_delete(app, key),
        AppMode::ConfirmSwap => handle_confirm_swap(app, key),
        AppMode::Help => {
            app.mode = AppMode::Normal;
        }
    }
}

fn handle_normal_key(
    app: &mut AppState,
    key: KeyEvent,
    tx: &mpsc::UnboundedSender<Event>,
) {
    match key.code {
        KeyCode::Char('q') => {
            app.should_quit = true;
        }
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.should_quit = true;
        }
        KeyCode::Char('j') | KeyCode::Down => {
            if !app.accounts.is_empty() {
                app.selected_index = (app.selected_index + 1) % app.accounts.len();
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if !app.accounts.is_empty() {
                app.selected_index = if app.selected_index == 0 {
                    app.accounts.len() - 1
                } else {
                    app.selected_index - 1
                };
            }
        }
        KeyCode::Char('r') => {
            crate::api::spawn_fetch_all(app, tx);
            crate::api::spawn_detect_logged_in(app, tx);
            app.set_status("Refreshing...".to_string());
        }
        KeyCode::Char('R') => {
            crate::api::spawn_fetch_one(app, app.selected_index, tx);
            app.set_status("Refreshing selected...".to_string());
        }
        KeyCode::Char('a') => {
            app.input_fields.clear();
            app.mode = AppMode::AddAccount;
        }
        KeyCode::Char('e') => {
            if let Some(account) = app.accounts.get(app.selected_index) {
                app.input_fields.name = account.config.name.clone();
                app.input_fields.org_id = account.config.org_id.clone();
                app.input_fields.session_key = account.cached_token.clone().unwrap_or_default();
                app.input_fields.focused_field = 0;
                app.mode = AppMode::EditAccount(app.selected_index);
            }
        }
        KeyCode::Char('d') | KeyCode::Char('x') => {
            if !app.accounts.is_empty() {
                app.mode = AppMode::ConfirmDelete;
            }
        }
        KeyCode::Char('s') | KeyCode::Enter => {
            if !app.accounts.is_empty() {
                app.mode = AppMode::ConfirmSwap;
            }
        }
        KeyCode::Char('i') => {
            crate::api::spawn_oauth_import(tx);
            app.set_status("Importing from Claude Code...".to_string());
        }
        KeyCode::Char('?') => {
            app.mode = AppMode::Help;
        }
        _ => {}
    }
}

fn handle_input_key(
    app: &mut AppState,
    key: KeyEvent,
    tx: &mpsc::UnboundedSender<Event>,
) {
    match key.code {
        KeyCode::Esc => {
            app.input_fields.clear();
            app.mode = AppMode::Normal;
        }
        KeyCode::Tab | KeyCode::Down => {
            app.input_fields.next_field();
        }
        KeyCode::BackTab | KeyCode::Up => {
            app.input_fields.prev_field();
        }
        KeyCode::Enter => {
            let name = app.input_fields.name.trim().to_string();
            let session_key = app.input_fields.session_key.trim().to_string();
            let org_id = app.input_fields.org_id.trim().to_string();

            if name.is_empty() || session_key.is_empty() || org_id.is_empty() {
                app.set_status("All fields are required".to_string());
                return;
            }

            match &app.mode {
                AppMode::AddAccount => {
                    if let Some(idx) = app.add_account(name, session_key, org_id) {
                        crate::api::spawn_fetch_one(app, idx, tx);
                    }
                }
                AppMode::EditAccount(index) => {
                    let idx = *index;
                    app.update_account(idx, name, session_key, org_id);
                    crate::api::spawn_fetch_one(app, idx, tx);
                }
                _ => {}
            }
            app.mode = AppMode::Normal;
        }
        KeyCode::Backspace => {
            app.input_fields.current_field_mut().pop();
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.input_fields.current_field_mut().clear();
        }
        KeyCode::Char(c) => {
            app.input_fields.current_field_mut().push(c);
        }
        _ => {}
    }
}

fn handle_confirm_delete(app: &mut AppState, key: KeyEvent) {
    match key.code {
        KeyCode::Char('y') | KeyCode::Enter => {
            app.delete_selected();
            app.mode = AppMode::Normal;
        }
        KeyCode::Char('n') | KeyCode::Esc => {
            app.mode = AppMode::Normal;
        }
        _ => {}
    }
}

fn handle_confirm_swap(app: &mut AppState, key: KeyEvent) {
    match key.code {
        KeyCode::Char('y') | KeyCode::Enter => {
            app.swap_to_selected();
            app.mode = AppMode::Normal;
        }
        KeyCode::Char('n') | KeyCode::Esc => {
            app.mode = AppMode::Normal;
        }
        _ => {}
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // -------------------------------------------------------------------------
    // Mock keyring: records all calls, configurable to fail on set
    // -------------------------------------------------------------------------
    struct MockKeyring {
        store: Mutex<std::collections::HashMap<String, String>>,
        calls: Mutex<Vec<(String, String)>>, // (operation, account_name)
        fail_on_set: Mutex<bool>,
    }

    impl MockKeyring {
        fn new() -> Self {
            Self {
                store: Mutex::new(std::collections::HashMap::new()),
                calls: Mutex::new(vec![]),
                fail_on_set: Mutex::new(false),
            }
        }

        fn with_fail_on_set() -> Self {
            let m = Self::new();
            *m.fail_on_set.lock().unwrap() = true;
            m
        }

        fn preload(&self, name: &str, key: &str) {
            self.store
                .lock()
                .unwrap()
                .insert(name.to_string(), key.to_string());
        }

        fn get_calls(&self) -> Vec<(String, String)> {
            self.calls.lock().unwrap().clone()
        }

        fn has_key(&self, name: &str) -> bool {
            self.store.lock().unwrap().contains_key(name)
        }
    }

    impl KeyringBackend for MockKeyring {
        fn get_session_key(&self, account_name: &str) -> Result<String, crate::error::TrackerError> {
            self.calls
                .lock()
                .unwrap()
                .push(("get".into(), account_name.into()));
            self.store
                .lock()
                .unwrap()
                .get(account_name)
                .cloned()
                .ok_or_else(|| {
                    crate::error::TrackerError::Keyring(format!("No key for '{account_name}'"))
                })
        }

        fn set_session_key(
            &self,
            account_name: &str,
            session_key: &str,
        ) -> Result<(), crate::error::TrackerError> {
            self.calls
                .lock()
                .unwrap()
                .push(("set".into(), account_name.into()));
            if *self.fail_on_set.lock().unwrap() {
                return Err(crate::error::TrackerError::Keyring(
                    "Simulated keyring write failure".into(),
                ));
            }
            self.store
                .lock()
                .unwrap()
                .insert(account_name.into(), session_key.into());
            Ok(())
        }

        fn delete_session_key(
            &self,
            account_name: &str,
        ) -> Result<(), crate::error::TrackerError> {
            self.calls
                .lock()
                .unwrap()
                .push(("delete".into(), account_name.into()));
            self.store.lock().unwrap().remove(account_name);
            Ok(())
        }
    }

    // -------------------------------------------------------------------------
    // Helper: build a test AppState without touching disk or real keyring
    // -------------------------------------------------------------------------
    fn test_app(names: &[&str], keyring: Arc<dyn KeyringBackend>) -> AppState {
        let accounts: Vec<AccountConfig> = names
            .iter()
            .map(|n| AccountConfig {
                name: n.to_string(),
                org_id: format!("org-{n}"),
                auth_method: AuthMethod::default(),
            })
            .collect();
        let config = Config {
            settings: crate::config::Settings::default(),
            accounts,
        };
        AppState::from_config(config, keyring)
    }

    // =========================================================================
    // FIX VERIFIED: add_account failure does not panic.
    //
    // Scenario: 0 accounts, keyring write fails, add_account returns None.
    // The if-let guard in handle_input_key prevents any index arithmetic
    // on the empty list.
    // =========================================================================
    #[test]
    fn add_account_failure_on_empty_list_does_not_panic() {
        let mock = Arc::new(MockKeyring::with_fail_on_set());
        let mut app = test_app(&[], mock);

        app.mode = AppMode::AddAccount;
        app.input_fields.name = "Test".to_string();
        app.input_fields.session_key = "sk-test".to_string();
        app.input_fields.org_id = "org-test".to_string();

        let (tx, _rx) = mpsc::unbounded_channel();
        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);

        handle_key(&mut app, key, &tx);

        assert_eq!(app.accounts.len(), 0, "No account should have been added");
        assert!(
            app.status_message.is_some(),
            "Should have an error status message"
        );
    }

    // =========================================================================
    // BUG 2: stale index after deletion misapplies usage result
    //
    // Scenario: accounts = [Alice, Bob, Charlie]. Fetch starts for all 3.
    // Bob (index 1) is deleted. accounts = [Alice, Charlie].
    // In-flight result for old index 1 (Bob) arrives.
    // BUG: result is applied to Charlie (now at index 1) instead of being
    // discarded.
    //
    // Expected: Charlie should NOT receive Bob's usage data.
    // =========================================================================
    #[test]
    fn bug2_usage_result_not_misapplied_after_deletion() {
        let mock = Arc::new(MockKeyring::new());
        let mut app = test_app(&["Alice", "Bob", "Charlie"], mock);

        // Simulate: fetch was started when Bob was at index 1
        // Then Bob is deleted
        app.accounts.remove(1); // accounts = [Alice, Charlie]

        // Stale in-flight result arrives for "Bob" (who no longer exists)
        let bobs_usage = UsageData {
            utilization: 99,
            resets_at: None,
            weekly_utilization: None,
            weekly_resets_at: None,
        };
        app.apply_usage_result("Bob", Ok(bobs_usage));

        // Charlie (now at index 1) must NOT have Bob's data
        let charlie = &app.accounts[1];
        assert_ne!(
            charlie.usage.as_ref().map(|u| u.utilization),
            Some(99),
            "Charlie must NOT receive Bob's stale usage result (utilization=99)"
        );
    }

    // =========================================================================
    // BUG 3: rename deletes old keyring entry before writing new one
    //
    // Scenario: account "OldName" has a stored key. User renames to "NewName".
    // update_account deletes "OldName" FIRST, then tries to set "NewName".
    // If set fails, the old credential is gone forever.
    //
    // Expected: old key should still exist when set_session_key fails.
    // =========================================================================
    #[test]
    fn bug3_rename_preserves_old_key_when_new_write_fails() {
        let mock = Arc::new(MockKeyring::new());
        mock.preload("OldName", "old-secret-key");

        // Verify precondition: old key exists
        assert!(mock.has_key("OldName"), "precondition: OldName key exists");

        let mut app = test_app(&["OldName"], mock.clone());

        // Now make set fail (simulating keychain write failure)
        *mock.fail_on_set.lock().unwrap() = true;

        // Try to rename OldName -> NewName
        app.update_account(
            0,
            "NewName".to_string(),
            "new-secret-key".to_string(),
            "org-new".to_string(),
        );

        // BUG: old key was deleted before the set attempt, so it's gone
        assert!(
            mock.has_key("OldName"),
            "Old key must still exist when new key write fails. \
             Calls: {:?}",
            mock.get_calls()
        );
    }

    // =========================================================================
    // FIX VERIFIED: Input fields cleared after edit cancel.
    //
    // When user presses Esc to cancel an edit, input_fields.clear() is called
    // so credentials don't linger in heap memory.
    // =========================================================================
    #[test]
    fn input_fields_cleared_on_edit_cancel() {
        let mock = Arc::new(MockKeyring::new());
        mock.preload("Alice", "alice-secret-key");
        let mut app = test_app(&["Alice"], mock);

        let (tx, _rx) = mpsc::unbounded_channel();

        // User presses 'e' to edit
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE),
            &tx,
        );
        assert_eq!(app.mode, AppMode::EditAccount(0));
        assert_eq!(
            app.input_fields.session_key, "alice-secret-key",
            "Session key loaded into input fields for editing"
        );

        // User presses Esc to cancel
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            &tx,
        );
        assert_eq!(app.mode, AppMode::Normal);

        // FIX: session key is cleared after cancel
        assert!(
            app.input_fields.session_key.is_empty(),
            "Session key must be cleared after edit cancel"
        );
        assert!(
            app.input_fields.name.is_empty(),
            "Name must be cleared after edit cancel"
        );
    }

    // =========================================================================
    // FIX VERIFIED: last_poll NOT updated when result is for a deleted account.
    //
    // apply_usage_result only sets last_poll when the named account exists
    // and actually receives the data. Discarded results don't affect the
    // status bar's "Last refresh" display.
    // =========================================================================
    #[test]
    fn last_poll_not_updated_when_result_discarded_for_deleted_account() {
        let mock = Arc::new(MockKeyring::new());
        let mut app = test_app(&["Alice"], mock);

        assert!(app.last_poll.is_none(), "Precondition: no poll yet");

        // Delete all accounts
        app.accounts.clear();

        // In-flight result for Alice arrives after she was deleted
        let usage = UsageData {
            utilization: 50,
            resets_at: None,
            weekly_utilization: None,
            weekly_resets_at: None,
        };
        app.apply_usage_result("Alice", Ok(usage));

        // FIX: last_poll is NOT set when no account received the data
        assert!(
            app.last_poll.is_none(),
            "last_poll must not be updated when result was discarded"
        );
    }

    // =========================================================================
    // FIX VERIFIED: last_poll IS updated when result is applied to an account.
    // =========================================================================
    #[test]
    fn last_poll_updated_when_result_applied_to_existing_account() {
        let mock = Arc::new(MockKeyring::new());
        let mut app = test_app(&["Alice"], mock);

        assert!(app.last_poll.is_none());

        let usage = UsageData {
            utilization: 50,
            resets_at: None,
            weekly_utilization: None,
            weekly_resets_at: None,
        };
        app.apply_usage_result("Alice", Ok(usage));

        assert!(
            app.last_poll.is_some(),
            "last_poll should be updated when result is applied to an existing account"
        );
    }

    // =========================================================================
    // INVARIANT: Error preserves existing usage data.
    //
    // This is the foundation of the "keep timers alive" feature. When a fetch
    // fails (e.g. token expired), the Err branch must NOT clear account.usage.
    // The UI relies on usage data surviving errors to keep showing countdowns.
    // =========================================================================
    #[test]
    fn error_preserves_existing_usage_data() {
        let mock = Arc::new(MockKeyring::new());
        let mut app = test_app(&["Alice"], mock);

        // First fetch succeeds — usage data is stored
        let usage = UsageData {
            utilization: 75,
            resets_at: Some(Utc::now() + chrono::Duration::hours(3)),
            weekly_utilization: Some(40),
            weekly_resets_at: Some(Utc::now() + chrono::Duration::days(5)),
        };
        app.apply_usage_result("Alice", Ok(usage));

        assert_eq!(app.accounts[0].usage.as_ref().unwrap().utilization, 75);
        assert_eq!(app.accounts[0].status, AccountStatus::Ok);

        // Second fetch fails — token expired
        app.apply_usage_result("Alice", Err("Expired — re-import (i)".to_string()));

        // Status is Error, but usage data MUST still be present
        assert!(
            matches!(app.accounts[0].status, AccountStatus::Error(_)),
            "Status should be Error"
        );
        assert!(
            app.accounts[0].usage.is_some(),
            "Usage data must survive an error — timers depend on this"
        );
        assert_eq!(
            app.accounts[0].usage.as_ref().unwrap().utilization,
            75,
            "Utilization must be unchanged after error"
        );
        assert!(
            app.accounts[0].usage.as_ref().unwrap().resets_at.is_some(),
            "resets_at must survive error — countdown timers depend on this"
        );
        assert_eq!(
            app.accounts[0].usage.as_ref().unwrap().weekly_utilization,
            Some(40),
            "Weekly utilization must survive error"
        );
    }

    // =========================================================================
    // INVARIANT: Multiple consecutive errors don't erode usage data.
    //
    // If the token stays expired for hours, every poll cycle produces an error.
    // Usage data must survive all of them. The timers tick locally and auto-
    // clear to 0% when resets_at passes — no fresh fetch needed.
    // =========================================================================
    #[test]
    fn consecutive_errors_preserve_usage() {
        let mock = Arc::new(MockKeyring::new());
        let mut app = test_app(&["Alice"], mock);

        // Successful fetch
        let usage = UsageData {
            utilization: 100,
            resets_at: Some(Utc::now() + chrono::Duration::hours(1)),
            weekly_utilization: Some(88),
            weekly_resets_at: Some(Utc::now() + chrono::Duration::days(3)),
        };
        app.apply_usage_result("Alice", Ok(usage));

        // 10 consecutive errors (simulating hours of expired token)
        for i in 0..10 {
            app.apply_usage_result("Alice", Err(format!("Expired attempt {}", i)));
        }

        assert!(
            app.accounts[0].usage.is_some(),
            "Usage must survive 10 consecutive errors"
        );
        assert_eq!(
            app.accounts[0].usage.as_ref().unwrap().utilization,
            100,
            "Utilization unchanged after 10 errors"
        );
        assert!(
            app.accounts[0].usage.as_ref().unwrap().resets_at.is_some(),
            "resets_at survives 10 errors"
        );
    }

    // =========================================================================
    // INVARIANT: Success after error replaces stale data with fresh data.
    //
    // When the user re-imports and the next fetch succeeds, the old cached
    // usage must be fully replaced — not merged or left stale.
    // =========================================================================
    #[test]
    fn success_after_error_replaces_stale_usage() {
        let mock = Arc::new(MockKeyring::new());
        let mut app = test_app(&["Alice"], mock);

        // Old successful fetch
        let old_usage = UsageData {
            utilization: 95,
            resets_at: Some(Utc::now() + chrono::Duration::hours(1)),
            weekly_utilization: Some(70),
            weekly_resets_at: Some(Utc::now() + chrono::Duration::days(2)),
        };
        app.apply_usage_result("Alice", Ok(old_usage));

        // Token expires, several errors
        app.apply_usage_result("Alice", Err("Expired".to_string()));
        app.apply_usage_result("Alice", Err("Expired".to_string()));

        // User re-imports, new fetch succeeds with different data
        let new_usage = UsageData {
            utilization: 10,
            resets_at: Some(Utc::now() + chrono::Duration::hours(5)),
            weekly_utilization: Some(20),
            weekly_resets_at: Some(Utc::now() + chrono::Duration::days(7)),
        };
        app.apply_usage_result("Alice", Ok(new_usage));

        assert_eq!(app.accounts[0].status, AccountStatus::Ok);
        assert_eq!(
            app.accounts[0].usage.as_ref().unwrap().utilization,
            10,
            "Fresh utilization must replace stale 95%"
        );
        assert_eq!(
            app.accounts[0].usage.as_ref().unwrap().weekly_utilization,
            Some(20),
            "Fresh weekly must replace stale 70%"
        );
    }

    // =========================================================================
    // EDGE CASE: First-ever fetch fails — no prior usage to preserve.
    //
    // Account just added/imported, first fetch fails immediately. There's no
    // cached usage data. UI should show placeholder row, not panic.
    // =========================================================================
    #[test]
    fn error_on_fresh_account_has_no_usage() {
        let mock = Arc::new(MockKeyring::new());
        let mut app = test_app(&["Alice"], mock);

        // Precondition: brand new account, no usage
        assert!(app.accounts[0].usage.is_none());

        // First fetch fails
        app.apply_usage_result("Alice", Err("No token cached — re-import (i)".to_string()));

        assert!(
            matches!(app.accounts[0].status, AccountStatus::Error(_)),
            "Status should be Error"
        );
        assert!(
            app.accounts[0].usage.is_none(),
            "No phantom usage data should appear — there was nothing to preserve"
        );
    }

    // =========================================================================
    // EDGE CASE: last_fetched not updated on error.
    //
    // last_fetched tracks the last SUCCESSFUL fetch. Errors should not
    // advance it. This matters if we recover to Ok — the staleness display
    // in the status column should reflect when data was actually refreshed.
    // =========================================================================
    #[test]
    fn last_fetched_not_advanced_by_error() {
        let mock = Arc::new(MockKeyring::new());
        let mut app = test_app(&["Alice"], mock);

        // Successful fetch
        let usage = UsageData {
            utilization: 50,
            resets_at: None,
            weekly_utilization: None,
            weekly_resets_at: None,
        };
        app.apply_usage_result("Alice", Ok(usage));

        let fetched_after_success = app.accounts[0].last_fetched.unwrap();

        // Error should not change last_fetched
        app.apply_usage_result("Alice", Err("Expired".to_string()));

        assert_eq!(
            app.accounts[0].last_fetched.unwrap(),
            fetched_after_success,
            "last_fetched must not advance on error — it tracks last successful fetch"
        );
    }

    // =========================================================================
    // EDGE CASE: import_oauth_account wipes usage (intentionally).
    //
    // Re-importing clears usage + resets status to Idle. If the subsequent
    // fetch fails, there's no stale data to show. This is by design — the
    // old timers belong to a different token session.
    // =========================================================================
    #[test]
    fn reimport_clears_usage_before_fresh_fetch() {
        let mock = Arc::new(MockKeyring::new());
        let mut app = test_app(&["Alice"], mock);
        app.accounts[0].config.auth_method = AuthMethod::OAuth;

        // Successful fetch populates usage
        let usage = UsageData {
            utilization: 80,
            resets_at: Some(Utc::now() + chrono::Duration::hours(2)),
            weekly_utilization: Some(60),
            weekly_resets_at: None,
        };
        app.apply_usage_result("Alice", Ok(usage));
        assert!(app.accounts[0].usage.is_some());

        // User re-imports the same account with a fresh token
        let import_data = OAuthImportData {
            name: "Alice".to_string(),
            org_id: "org-Alice".to_string(),
            access_token: "fresh-token-xyz".to_string(),
        };
        app.import_oauth_account(import_data);

        // Usage is wiped — old timers don't apply to the new token session
        assert!(
            app.accounts[0].usage.is_none(),
            "Usage must be cleared on re-import"
        );
        assert_eq!(
            app.accounts[0].status,
            AccountStatus::Idle,
            "Status must reset to Idle on re-import"
        );
        assert_eq!(
            app.accounts[0].cached_token.as_deref(),
            Some("fresh-token-xyz"),
            "Cached token must be updated"
        );
    }
}
