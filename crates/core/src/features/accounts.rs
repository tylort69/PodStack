//! Accounts: EVE's API doesn't say which characters share an account, so the player groups them here (Queue Health
//! shows when a whole account has nothing training). Stored in the `accounts` document.

use crate::store;
use crate::{Core, Event};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::sync::LazyLock;

pub const DOC: &str = "accounts";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AccountInfo {
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Account {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct AccountsDoc {
    /// account id -> account
    #[serde(default)]
    pub accounts: BTreeMap<String, AccountInfo>,
    /// character id -> account id
    #[serde(default)]
    pub assignments: BTreeMap<i64, String>,
}

/// Compares names the way people sort them: "Account 2" before "Account 10", ignoring case.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let mut na = String::new();
                while let Some(c) = a.peek().copied().filter(char::is_ascii_digit) {
                    na.push(c);
                    a.next();
                }
                let mut nb = String::new();
                while let Some(c) = b.peek().copied().filter(char::is_ascii_digit) {
                    nb.push(c);
                    b.next();
                }
                let (ta, tb) = (na.trim_start_matches('0'), nb.trim_start_matches('0'));
                let o = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                if o != Ordering::Equal {
                    return o;
                }
            }
            (Some(x), Some(y)) => {
                let o = x.to_lowercase().cmp(y.to_lowercase());
                if o != Ordering::Equal {
                    return o;
                }
                a.next();
                b.next();
            }
        }
    }
}

impl AccountsDoc {
    /// Accounts sorted by name.
    pub fn list(&self) -> Vec<Account> {
        let mut v: Vec<Account> = self.accounts.iter().map(|(id, a)| Account { id: id.clone(), name: a.name.clone() }).collect();
        v.sort_by(|a, b| natural_cmp(&a.name, &b.name).then_with(|| a.id.cmp(&b.id)));
        v
    }

    /// The account a character is assigned to (only if that account exists).
    pub fn account_of(&self, character_id: i64) -> Option<&str> {
        self.assignments.get(&character_id).map(String::as_str).filter(|id| self.accounts.contains_key(*id))
    }

    pub fn create(&mut self, id: String, name: &str) {
        self.accounts.insert(id, AccountInfo { name: name.trim().to_string() });
    }

    pub fn rename(&mut self, id: &str, name: &str) -> bool {
        match self.accounts.get_mut(id) {
            Some(a) => {
                a.name = name.trim().to_string();
                true
            }
            None => false,
        }
    }

    /// Deleting an account leaves its characters unassigned.
    pub fn delete(&mut self, id: &str) {
        self.accounts.remove(id);
        self.assignments.retain(|_, a| a != id);
    }

    /// `None` (or an unknown account) unassigns the character.
    pub fn assign(&mut self, character_id: i64, account: Option<&str>) {
        match account.filter(|a| self.accounts.contains_key(*a)) {
            Some(a) => {
                self.assignments.insert(character_id, a.to_string());
            }
            None => {
                self.assignments.remove(&character_id);
            }
        }
    }
}

static ACCOUNTS: LazyLock<Mutex<AccountsDoc>> = LazyLock::new(|| Mutex::new(store::load_doc(DOC)));

fn change(core: &Core, f: impl FnOnce(&mut AccountsDoc)) {
    let snapshot = {
        let mut doc = ACCOUNTS.lock();
        let before = doc.clone();
        f(&mut doc);
        if *doc == before {
            return;
        }
        doc.clone()
    };
    store::save_doc(DOC, &snapshot);
    core.emit(Event::Doc(DOC.into()));
}

/// Accounts sorted by name.
pub fn accounts() -> Vec<Account> {
    ACCOUNTS.lock().list()
}

pub fn account_of(character_id: i64) -> Option<String> {
    ACCOUNTS.lock().account_of(character_id).map(str::to_string)
}

/// Creates an account and returns its id.
pub fn create(core: &Core, name: &str) -> String {
    let id = format!("{:x}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default());
    let id2 = id.clone();
    change(core, move |d| d.create(id2, name));
    id
}

pub fn rename(core: &Core, id: &str, name: &str) {
    change(core, |d| {
        d.rename(id, name);
    });
}

pub fn delete(core: &Core, id: &str) {
    change(core, |d| d.delete(id));
}

pub fn assign(core: &Core, character_id: i64, account: Option<&str>) {
    change(core, |d| d.assign(character_id, account));
}

/// Forgets a removed character's assignment.
pub fn forget_character(core: &Core, character_id: i64) {
    assign(core, character_id, None);
}

/// The whole document (backups).
pub fn export_all() -> AccountsDoc {
    ACCOUNTS.lock().clone()
}

/// Replaces the whole document (backup restore).
pub fn import_all(core: &Core, doc: AccountsDoc) {
    change(core, move |d| *d = doc);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_order() {
        let mut v = vec!["Account 10", "account 2", "Account 1", "Alts", "Account 02b"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, vec!["Account 1", "account 2", "Account 02b", "Account 10", "Alts"]);
    }

    #[test]
    fn assign_rename_delete() {
        let mut d = AccountsDoc::default();
        d.create("a".into(), " Main ");
        d.create("b".into(), "Farms");
        assert_eq!(d.list().iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), vec!["Farms", "Main"]);
        d.assign(1, Some("a"));
        d.assign(2, Some("a"));
        d.assign(3, Some("missing"));
        assert_eq!(d.account_of(1), Some("a"));
        assert_eq!(d.account_of(3), None);
        assert!(d.rename("a", "Primary"));
        assert!(!d.rename("zzz", "x"));
        d.assign(2, None);
        assert_eq!(d.account_of(2), None);
        d.delete("a");
        assert_eq!(d.account_of(1), None);
        assert!(d.assignments.is_empty());
        assert_eq!(d.list().len(), 1);
    }

    #[test]
    fn json_shape() {
        let mut d = AccountsDoc::default();
        d.create("x".into(), "One");
        d.assign(42, Some("x"));
        let s = serde_json::to_string(&d).unwrap();
        assert_eq!(s, r#"{"accounts":{"x":{"name":"One"}},"assignments":{"42":"x"}}"#);
        let back: AccountsDoc = serde_json::from_str(&s).unwrap();
        assert_eq!(back, d);
    }
}
