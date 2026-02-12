import { invoke } from "@tauri-apps/api/core";

// ============================================================================
// Types (must match Rust lib.rs ApiKeySummary, CreateApiKeyResponse, AuthState)
// ============================================================================

export type AuthMode = "public" | "protected";

export type ApiKeySource = "legacy" | "user";

export const SCOPES = [
  "admin",
  "system:read",
  "usage:read",
  "stats:read",
  "media:read",
  "media:control",
  "processes:read",
  "processes:control",
  "power:control",
  "stream:read",
  "ws:connect",
] as const;

export type Scope = (typeof SCOPES)[number];

export interface ApiKeySummary {
  id: string;
  name: string;
  hint: string;
  scopes: string[];
  created_at: number;
  expires_at: number | null;
  revoked_at: number | null;
  source: ApiKeySource;
}

export interface CreateApiKeyResponse {
  key: string;
  record: ApiKeySummary;
}

export interface AuthInfo {
  mode: AuthMode;
  keys: ApiKeySummary[];
}

// ============================================================================
// Auth State
// ============================================================================

/**
 * Get auth mode and key list. Combines set_auth_mode read + list_api_keys.
 */
export const getAuthInfo = async (): Promise<AuthInfo> => {
  try {
    const [keys, config] = await Promise.all([
      invoke<ApiKeySummary[]>("list_api_keys"),
      invoke<{ auth: { enabled: boolean } }>("get_config"),
    ]);
    return {
      mode: config.auth.enabled ? "protected" : "public",
      keys,
    };
  } catch (e) {
    console.error("Failed to get auth info:", e);
    return { mode: "public", keys: [] };
  }
};

/**
 * Set auth mode (public | protected). Returns updated config.
 */
export const setAuthMode = async (mode: AuthMode) => {
  try {
    await invoke("set_auth_mode", { mode });
    return true;
  } catch (e) {
    console.error("Failed to set auth mode:", e);
    return false;
  }
};

// ============================================================================
// Key Management
// ============================================================================

/**
 * List all API keys (without secrets).
 */
export const listApiKeys = async (): Promise<ApiKeySummary[]> => {
  try {
    return await invoke<ApiKeySummary[]>("list_api_keys");
  } catch (e) {
    console.error("Failed to list API keys:", e);
    return [];
  }
};

/**
 * Create a new API key. Returns the key (only shown once) and its summary record.
 */
export const createApiKey = async (
  name?: string,
  scopes: string[] = ["admin"],
  expiresAt?: number,
): Promise<CreateApiKeyResponse | null> => {
  try {
    return await invoke<CreateApiKeyResponse>("create_api_key", {
      name: name ?? null,
      scopes,
      expiresAt: expiresAt ?? null,
    });
  } catch (e) {
    console.error("Failed to create API key:", e);
    return null;
  }
};

/**
 * Revoke an API key by ID.
 */
export const revokeApiKey = async (id: string): Promise<boolean> => {
  try {
    return await invoke<boolean>("revoke_api_key", { id });
  } catch (e) {
    console.error("Failed to revoke API key:", e);
    return false;
  }
};

/**
 * Update scopes for an existing API key.
 */
export const updateApiKeyScopes = async (
  id: string,
  scopes: string[],
): Promise<boolean> => {
  try {
    return await invoke<boolean>("update_api_key_scopes", { id, scopes });
  } catch (e) {
    console.error("Failed to update API key scopes:", e);
    return false;
  }
};

/**
 * Update expiration for an existing API key. Pass null to remove expiration.
 */
export const updateApiKeyExpiration = async (
  id: string,
  expiresAt: number | null,
): Promise<boolean> => {
  try {
    return await invoke<boolean>("update_api_key_expiration", { id, expiresAt });
  } catch (e) {
    console.error("Failed to update API key expiration:", e);
    return false;
  }
};

// ============================================================================
// Legacy Config Auth (bridges old config.auth model — migrate UI to key mgmt)
// ============================================================================

import type { Config } from "./backend";

export const toggleAuth = async (): Promise<Config | null> => {
  try {
    return await invoke<Config>("toggle_auth");
  } catch (e) {
    console.error("Failed to toggle auth:", e);
    return null;
  }
};

export const updateLegacyApiKey = async (
  apiKey: string | null,
): Promise<Config | null> => {
  try {
    return await invoke<Config>("update_api_key", { apiKey });
  } catch (e) {
    console.error("Failed to update API key:", e);
    return null;
  }
};

export const generateLegacyApiKey = async (): Promise<Config | null> => {
  try {
    return await invoke<Config>("generate_api_key");
  } catch (e) {
    console.error("Failed to generate API key:", e);
    return null;
  }
};
