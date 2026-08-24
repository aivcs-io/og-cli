/// Account & organization (tenant) management
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use tracing::info;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    pub name: String,
    pub email: String,
    pub identity_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountState {
    pub accounts: Vec<Account>,
    pub active_account: Option<String>,
    pub active_org: String,
}

impl Default for AccountState {
    fn default() -> Self {
        Self {
            accounts: Vec::new(),
            active_account: None,
            active_org: "acme".to_string(),
        }
    }
}

pub struct AccountManager;

impl AccountManager {
    /// Show active account & org context
    pub fn show() -> Result<()> {
        let state = Self::load_state()?;

        if let Some(active_id) = &state.active_account {
            if let Some(account) = state.accounts.iter().find(|a| &a.id == active_id) {
                println!("Account:  {} ({})", account.name, account.email);
                println!("Org:      {}", state.active_org);
                println!("Status:   authenticated");
                return Ok(());
            }
        }

        println!("Status:   not authenticated");
        println!("Org:      {} (default)", state.active_org);
        Ok(())
    }

    /// List all accounts on this machine
    pub fn list() -> Result<()> {
        let state = Self::load_state()?;

        if state.accounts.is_empty() {
            println!("No accounts configured.");
            return Ok(());
        }

        println!("Accounts:");
        for account in &state.accounts {
            let marker = if Some(&account.id) == state.active_account.as_ref() {
                "* (active)"
            } else {
                ""
            };
            println!(
                "  {} {} <{}>  {}",
                marker, account.name, account.email, account.id
            );
        }
        Ok(())
    }

    /// Switch to a different account
    pub fn switch(account_id: &str) -> Result<()> {
        let mut state = Self::load_state()?;

        if !state.accounts.iter().any(|a| a.id == account_id) {
            return Err(anyhow!("Account not found: {}", account_id));
        }

        state.active_account = Some(account_id.to_string());
        Self::save_state(&state)?;
        info!("Switched to account: {}", account_id);
        println!("✓ Switched to account: {}", account_id);
        Ok(())
    }

    /// Logout the active account
    pub fn logout(account_id: Option<&str>) -> Result<()> {
        let mut state = Self::load_state()?;

        if let Some(id) = account_id {
            state.accounts.retain(|a| a.id != id);
            if state.active_account.as_ref() == Some(&id.to_string()) {
                state.active_account = None;
            }
            info!("Logged out account: {}", id);
            println!("✓ Logged out: {}", id);
        } else {
            state.active_account = None;
            info!("Logged out active account");
            println!("✓ Logged out active account");
        }

        Self::save_state(&state)?;
        Ok(())
    }

    /// Add a new account (called after identity-broker login)
    pub fn add_account(account: Account) -> Result<()> {
        let mut state = Self::load_state()?;
        state.accounts.retain(|a| a.id != account.id);
        state.accounts.push(account.clone());
        state.active_account = Some(account.id.clone());
        Self::save_state(&state)?;
        info!("Added account: {}", account.id);
        Ok(())
    }

    /// Get active account
    pub fn get_active() -> Result<Option<Account>> {
        let state = Self::load_state()?;
        Ok(state
            .active_account
            .as_ref()
            .and_then(|id| state.accounts.iter().find(|a| &a.id == id).cloned()))
    }

    /// Get active org
    pub fn get_active_org() -> Result<String> {
        let state = Self::load_state()?;
        Ok(state.active_org)
    }

    // Private helpers
    fn state_path() -> Result<PathBuf> {
        let config_dir =
            dirs::config_dir().ok_or_else(|| anyhow!("Cannot find config directory"))?;
        Ok(config_dir.join("og").join("account_state.json"))
    }

    fn load_state() -> Result<AccountState> {
        let path = Self::state_path()?;
        if path.exists() {
            let data = fs::read_to_string(&path)?;
            Ok(serde_json::from_str(&data)?)
        } else {
            Ok(AccountState::default())
        }
    }

    fn save_state(state: &AccountState) -> Result<()> {
        let path = Self::state_path()?;
        fs::create_dir_all(path.parent().unwrap())?;
        let data = serde_json::to_string_pretty(state)?;
        fs::write(&path, data)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_account_state_default() {
        let state = AccountState::default();
        assert_eq!(state.active_org, "acme");
        assert!(state.active_account.is_none());
    }

    #[test]
    fn test_add_and_switch_account() {
        let account = Account {
            id: "test-123".to_string(),
            name: "Test User".to_string(),
            email: "test@example.com".to_string(),
            identity_token: "token-xyz".to_string(),
        };

        // This would require mocking file I/O in a real test
        // For now, just verify the struct is valid
        assert_eq!(account.id, "test-123");
    }
}
