Great idea. Here's a clean structure:

cntrl/
├── packages/
│ ├── ui/ # Shared React components
│ │ └── src/
│ │ ├── MediaCard.tsx
│ │ ├── ProcessList.tsx
│ │ ├── SystemStats.tsx
│ │ └── ...
│ │
│ ├── sdk/ # Cntrl Bridge client SDK
│ │ └── src/
│ │ ├── client.ts # REST client
│ │ ├── websocket.ts # WS client
│ │ └── types.ts # Shared types
│ │
│ └── hooks/ # TanStack Query hooks
│ └── src/
│ ├── useSystem.ts
│ ├── useMedia.ts
│ ├── useProcesses.ts
│ └── useCntrlWS.ts # WebSocket subscription hook
│
├── apps/
│ ├── pi/ # RPi Dashboard
│ │ ├── app/ # TanStack Start
│ │ └── db/ # SQLite, Drizzle/etc
│ │
│ └── web/ # Hosted (static PWA)
│ └── src/ # Vite + React, localStorage config
│
├── package.json
├── pnpm-workspace.yaml
└── turbo.json

Key Abstractions

@cntrl/sdk - The core client:
// packages/sdk/src/client.ts
export class CntrlClient {
constructor(private config: { host: string; apiKey?: string }) {}

    async getSystem() { ... }
    async getMedia() { ... }
    async controlMedia(action: string) { ... }
    // etc

}

@cntrl/hooks - TanStack Query hooks:
// packages/hooks/src/useMedia.ts
export function useMedia(client: CntrlClient) {
return useQuery({
queryKey: ['media'],
queryFn: () => client.getMedia(),
});
}

// WebSocket hook
export function useCntrlWS(host: string, topics: string[]) {
// Returns reactive state, auto-reconnect, etc.
}

@cntrl/ui - Shared components:
// packages/ui/src/MediaCard.tsx
export function MediaCard({ data, onControl }: MediaCardProps) {
// Pure presentational component
}

App Differences
┌────────────────┬────────────────────────────┬─────────────────────────────┐
│ Concern │ apps/pi │ apps/web │
├────────────────┼────────────────────────────┼─────────────────────────────┤
│ Config storage │ SQLite │ localStorage (encrypted) │
├────────────────┼────────────────────────────┼─────────────────────────────┤
│ Device list │ Shared across visitors │ Per-browser │
├────────────────┼────────────────────────────┼─────────────────────────────┤
│ Auth │ Optional (trusted network) │ Passphrase to unlock config │
├────────────────┼────────────────────────────┼─────────────────────────────┤
│ Rendering │ SSR via TanStack Start │ Static SPA │
├────────────────┼────────────────────────────┼─────────────────────────────┤
│ Hosting │ Self-hosted on RPi │ Cloudflare Pages / Vercel │
└────────────────┴────────────────────────────┴─────────────────────────────┘

---

Bearer Auth - Is It Standard?

Yes, Bearer is the standard. You're doing it right.

Authorization: Bearer <token> is defined in https://datatracker.ietf.org/doc/html/rfc6750 and used by:

- GitHub API
- Stripe API
- Discord API
- Literally every modern REST API

Comparison
┌────────────────┬────────────────────────────────────────┬─────────────────────────────────────┐
│ Method │ Format │ Verdict │
├────────────────┼────────────────────────────────────────┼─────────────────────────────────────┤
│ Bearer │ Authorization: Bearer xxx │ Industry standard │
├────────────────┼────────────────────────────────────────┼─────────────────────────────────────┤
│ API Key header │ X-API-Key: xxx │ Also common, slightly less standard │
├────────────────┼────────────────────────────────────────┼─────────────────────────────────────┤
│ Basic Auth │ Authorization: Basic base64(user:pass) │ Outdated for APIs │
├────────────────┼────────────────────────────────────────┼─────────────────────────────────────┤
│ OAuth │ Complex flow │ Overkill for local tool │
└────────────────┴────────────────────────────────────────┴─────────────────────────────────────┘
One Addition to Consider

For WebSocket and SSE (where headers are awkward in browsers), you already support this but could document:

ws://host:9990/api/ws?api_key=xxx

Query param fallback for browser clients. Keep Bearer as primary, query param as convenience.

---

TL;DR:

- Monorepo with packages/{sdk,hooks,ui} and apps/{pi,web} is the right call
- Bearer auth is perfectly standard - community will expect it
- TanStack Start for pi app works well, Vite for web (static)
