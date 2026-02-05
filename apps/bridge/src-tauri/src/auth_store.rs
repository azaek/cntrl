use crate::config::AppConfig;
use argon2::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

const AUTH_STORE_VERSION: u32 = 1;
const KEYRING_SERVICE: &str = "cntrl.bridge";
const KEYRING_ACCOUNT: &str = "auth_state";

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuthMode {
    Public,
    Protected,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApiKeySource {
    Legacy,
    User,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub struct ApiKeyRecord {
    pub id: String,
    pub name: String,
    pub hash: String,
    pub hint: String,
    pub scopes: Vec<String>,
    pub created_at: i64,
    pub expires_at: Option<i64>,
    pub last_used_at: Option<i64>,
    pub revoked_at: Option<i64>,
    pub source: ApiKeySource,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub struct AuthState {
    pub version: u32,
    pub mode: AuthMode,
    pub keys: Vec<ApiKeyRecord>,
}

impl Default for AuthState {
    fn default() -> Self {
        Self {
            version: AUTH_STORE_VERSION,
            mode: AuthMode::Public,
            keys: Vec::new(),
        }
    }
}

pub struct AuthStateLoad {
    pub state: AuthState,
    pub existed: bool,
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn keyring_entry() -> Result<keyring::Entry, String> {
    keyring::Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT).map_err(|e| e.to_string())
}

pub fn load_auth_state() -> AuthStateLoad {
    let entry = match keyring_entry() {
        Ok(e) => e,
        Err(err) => {
            println!("[auth_store] Keyring entry error: {}", err);
            return AuthStateLoad {
                state: AuthState::default(),
                existed: false,
            };
        }
    };

    match entry.get_password() {
        Ok(secret) => match serde_json::from_str::<AuthState>(&secret) {
            Ok(state) => AuthStateLoad {
                state,
                existed: true,
            },
            Err(err) => {
                println!("[auth_store] Failed to parse auth state: {}", err);
                AuthStateLoad {
                    state: AuthState::default(),
                    existed: true,
                }
            }
        },
        Err(_) => AuthStateLoad {
            state: AuthState::default(),
            existed: false,
        },
    }
}

pub fn save_auth_state(state: &AuthState) -> Result<(), String> {
    let entry = keyring_entry()?;
    let payload = serde_json::to_string(state).map_err(|e| e.to_string())?;
    entry.set_password(&payload).map_err(|e| e.to_string())
}

pub fn load_or_migrate(config: &AppConfig) -> AuthState {
    let load = load_auth_state();
    if load.existed {
        return load.state;
    }

    let mut state = AuthState::default();
    state.mode = if config.auth.enabled {
        AuthMode::Protected
    } else {
        AuthMode::Public
    };

    if let Some(token) = config
        .auth
        .api_key
        .as_ref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    {
        if let Ok(record) = create_key_record_from_token(
            &token,
            "Legacy Key",
            vec!["admin".to_string()],
            ApiKeySource::Legacy,
            None,
        ) {
            state.keys.push(record);
        }
    }

    if let Err(err) = save_auth_state(&state) {
        println!("[auth_store] Failed to save auth state: {}", err);
    }

    state
}

pub fn create_api_key(
    name: String,
    scopes: Vec<String>,
    expires_at: Option<i64>,
    source: ApiKeySource,
) -> Result<(ApiKeyRecord, String), String> {
    let token = generate_token();
    let record = create_key_record_from_token(&token, &name, scopes, source, expires_at)?;
    Ok((record, token))
}

pub fn create_key_record_from_token(
    token: &str,
    name: &str,
    scopes: Vec<String>,
    source: ApiKeySource,
    expires_at: Option<i64>,
) -> Result<ApiKeyRecord, String> {
    let hash = hash_token(token)?;
    Ok(ApiKeyRecord {
        id: generate_id(),
        name: name.to_string(),
        hash,
        hint: key_hint(token),
        scopes,
        created_at: now_unix(),
        expires_at,
        last_used_at: None,
        revoked_at: None,
        source,
    })
}

pub fn find_active_record(state: &AuthState, token: &str) -> Option<ApiKeyRecord> {
    let now = now_unix();

    for record in &state.keys {
        if record.revoked_at.is_some() {
            continue;
        }
        if let Some(exp) = record.expires_at {
            if exp <= now {
                continue;
            }
        }
        if verify_hash(token, &record.hash) {
            return Some(record.clone());
        }
    }

    None
}

pub fn has_scope(record: &ApiKeyRecord, required: &str) -> bool {
    record.scopes.iter().any(|s| s == "admin" || s == required)
}

pub fn revoke_key(state: &mut AuthState, id: &str) -> bool {
    let now = now_unix();
    for record in &mut state.keys {
        if record.id == id {
            record.revoked_at = Some(now);
            return true;
        }
    }
    false
}

pub fn update_key_scopes(state: &mut AuthState, id: &str, scopes: Vec<String>) -> bool {
    for record in &mut state.keys {
        if record.id == id {
            record.scopes = scopes;
            return true;
        }
    }
    false
}

pub fn update_key_expiration(state: &mut AuthState, id: &str, expires_at: Option<i64>) -> bool {
    for record in &mut state.keys {
        if record.id == id {
            record.expires_at = expires_at;
            return true;
        }
    }
    false
}

fn hash_token(token: &str) -> Result<String, String> {
    let salt = SaltString::generate(&mut rand_core::OsRng);
    let argon2 = Argon2::default();
    let hash = argon2
        .hash_password(token.as_bytes(), &salt)
        .map_err(|e| e.to_string())?
        .to_string();
    Ok(hash)
}

fn verify_hash(token: &str, hash: &str) -> bool {
    let parsed = match PasswordHash::new(hash) {
        Ok(p) => p,
        Err(_) => return false,
    };
    Argon2::default()
        .verify_password(token.as_bytes(), &parsed)
        .is_ok()
}

fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let mut hex = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        hex.push_str(&format!("{:02x}", b));
    }
    format!("ck_{}", hex)
}

fn generate_id() -> String {
    let mut bytes = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let mut hex = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        hex.push_str(&format!("{:02x}", b));
    }
    format!("key_{}", hex)
}

fn key_hint(token: &str) -> String {
    if token.len() <= 4 {
        token.to_string()
    } else {
        token[token.len() - 4..].to_string()
    }
}
