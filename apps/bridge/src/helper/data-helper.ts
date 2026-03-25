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

export const getLocalIp = (_ips: string[]) => {
    return (
        _ips.find((ip) => {
            if (ip.startsWith("192.168.")) return true;
            if (ip.startsWith("10.")) return true;
            const parts = ip.split(".");
            if (parts.length === 4 && parts[0] === "172") {
                const second = parseInt(parts[1], 10);
                if (second >= 16 && second <= 31) return true;
            }
            return false;
        }) ??
        _ips.find((ip) => ip !== "127.0.0.1" && ip !== "::1") ??
        "localhost"
    );
};
