import { SCOPES } from "../lib/auth";

export const sanitizeScopes = (_scopes: string[]) => {
  let scopes = [..._scopes];
  if (scopes.includes("usage:read") && scopes.includes("stats:read")) {
    scopes = scopes.filter((perm) => perm !== "usage:read");
  }
  // Admin check
  if (scopes.includes("admin")) {
    return [SCOPES[0]];
  }
  return scopes;
};
