use std::sync::Arc;

use chrono::{DateTime, Utc};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tokio::sync::mpsc;

use crate::config::{self, AccountConfig, AuthMethod, Config};
use crate::event::{Event, OAuthImportData};
use crate::keyring_store::KeyringBackend;
use crate::swap;

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
    Fetching,
    Ok,
    Error(String),
}

#[derive(Debug, Clone)]
pub struct AccountState {
    pub config: AccountConfig,
    pub usage: Option<UsageData>,
    pub status: AccountStatus,
    pub last_fetched: Option<DateTime<Utc>>,
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
            _ => &mut self.name,
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
    config: Config,
}

impl AppState {
    pub fn from_config(config: Config, keyring: Arc<dyn KeyringBackend>) -> Self {
        let accounts: Vec<AccountState> = config
            .accounts
            .iter()
            .map(|ac| AccountState {
                config: ac.clone(),
                usage: None,
                status: AccountStatus::Idle,
                last_fetched: None,
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
            poll_interval_secs: config.settings.poll_interval_secs,
            keyring,
            config,
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
        }
        // If account was deleted while fetch was in flight, result is silently discarded.
        self.last_poll = Some(Utc::now());
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
        self.config.accounts = self
            .accounts
            .iter()
            .map(|a| a.config.clone())
            .collect();
        self.config.settings.active_account = self.active_account_index;
        if let Err(e) = config::save(&self.config) {
            self.set_status(format!("Failed to save config: {e}"));
        }
    }

    /// Returns Some(index) on success, None on failure.
    fn add_account(&mut self, name: String, session_key: String, org_id: String) -> Option<usize> {
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
        });
        self.save_config();
        self.set_status("Account added".to_string());
        Some(self.accounts.len() - 1)
    }

    /// Write new key first. Only delete old key after new key write succeeds.
    fn update_account(&mut self, index: usize, name: String, session_key: String, org_id: String) {
        if let Some(account) = self.accounts.get_mut(index) {
            let old_name = account.config.name.clone();
            let name_changed = old_name != name;

            // Write new key FIRST -- if this fails, old key is preserved
            if let Err(e) = self.keyring.set_session_key(&name, &session_key) {
                self.set_status(format!("Keyring error: {e}"));
                return;
            }

            // Only delete old key AFTER new key is safely stored
            if name_changed {
                let _ = self.keyring.delete_session_key(&old_name);
            }

            account.config.name = name;
            account.config.org_id = org_id;
            account.usage = None;
            account.status = AccountStatus::Idle;
            self.save_config();
            self.set_status("Account updated".to_string());
        }
    }

    fn delete_selected(&mut self) {
        if self.selected_index < self.accounts.len() {
            let name = self.accounts[self.selected_index].config.name.clone();
            let _ = self.keyring.delete_session_key(&name);
            self.accounts.remove(self.selected_index);

            if self.selected_index >= self.accounts.len() && !self.accounts.is_empty() {
                self.selected_index = self.accounts.len() - 1;
            }
            if self.active_account_index >= self.accounts.len() && !self.accounts.is_empty() {
                self.active_account_index = self.accounts.len() - 1;
            }
            self.save_config();
            self.set_status("Account deleted".to_string());
        }
    }

    /// Import an OAuth account from Claude Code. If an account with the same name
    /// already exists, update its credentials. Otherwise, add a new account.
    /// Returns the account index on success.
    pub fn import_oauth_account(&mut self, data: OAuthImportData) -> Option<usize> {
        // Store the OAuth credential JSON in our keyring
        if let Err(e) = self.keyring.set_session_key(&data.name, &data.credential_json) {
            self.set_status(format!("Keyring error: {e}"));
            return None;
        }

        // Check if account already exists (by name)
        if let Some(pos) = self.accounts.iter().position(|a| a.config.name == data.name) {
            self.accounts[pos].config.org_id = data.org_id;
            self.accounts[pos].config.auth_method = AuthMethod::OAuth;
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
        });
        self.save_config();
        self.set_status(format!("Imported OAuth account '{}'", data.name));
        Some(self.accounts.len() - 1)
    }

    fn swap_to_selected(&mut self) {
        if self.selected_index < self.accounts.len() {
            let name = self.accounts[self.selected_index].config.name.clone();
            let auth_method = self.accounts[self.selected_index].config.auth_method.clone();

            match &auth_method {
                AuthMethod::OAuth => {
                    match swap::swap_claude_code_credential(
                        self.keyring.as_ref(),
                        &name,
                        &auth_method,
                    ) {
                        Ok(()) => {
                            self.active_account_index = self.selected_index;
                            self.save_config();
                            self.set_status(format!("Swapped to '{}' — Claude Code will use it now", name));
                        }
                        Err(e) => {
                            self.set_status(format!("Swap failed: {e}"));
                        }
                    }
                }
                AuthMethod::SessionKey => {
                    let org_id = self.accounts[self.selected_index].config.org_id.clone();
                    match self.keyring.get_session_key(&name) {
                        Ok(session_key) => {
                            match swap::write_active_session(&session_key, &org_id) {
                                Ok(()) => {
                                    self.active_account_index = self.selected_index;
                                    self.save_config();
                                    self.set_status(format!("Swapped to '{}'", name));
                                }
                                Err(e) => {
                                    self.set_status(format!("Swap failed: {e}"));
                                }
                            }
                        }
                        Err(e) => {
                            self.set_status(format!("Cannot swap - {e}"));
                        }
                    }
                }
            }
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
        KeyCode::Char('q') | KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.should_quit = true;
        }
        KeyCode::Char('q') => {
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
                let name = account.config.name.clone();
                app.input_fields.name = name.clone();
                app.input_fields.org_id = account.config.org_id.clone();
                app.input_fields.session_key = app
                    .keyring
                    .get_session_key(&name)
                    .unwrap_or_default();
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
    // BUG 1: add_account failure causes panic via len()-1 underflow
    //
    // Scenario: 0 accounts, keyring write fails, add_account returns without
    // pushing. handle_input_key then does `accounts.len() - 1` on a usize = 0
    // which panics with underflow.
    //
    // Expected: should NOT panic; should set an error status message instead.
    // =========================================================================
    #[test]
    fn bug1_add_account_failure_on_empty_list_must_not_panic() {
        let mock = Arc::new(MockKeyring::with_fail_on_set());
        let mut app = test_app(&[], mock);

        // Simulate user filling in the add-account form
        app.mode = AppMode::AddAccount;
        app.input_fields.name = "Test".to_string();
        app.input_fields.session_key = "sk-test".to_string();
        app.input_fields.org_id = "org-test".to_string();

        // Press Enter to submit -- this triggers handle_input_key
        let (tx, _rx) = mpsc::unbounded_channel();
        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);

        // BUG: this panics because add_account fails, len()=0, 0-1 underflows
        handle_key(&mut app, key, &tx);

        // If we reach here, no panic occurred
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
}
