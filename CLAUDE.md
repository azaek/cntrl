# CLAUDE.md

Cntrl Bridge: The programmable, open-source API for multi-OS desktop control.

This monorepo contains the bridge app, SDK, UI components, and consumer apps.

## Commands

```bash
# Development
pnpm dev              # All apps (except bridge)
pnpm dev:bridge       # Tauri bridge (requires Rust)
pnpm dev:hub          # Hub dashboard (port 3001)
pnpm dev:web          # Landing page (port 3000)
pnpm dev:docs         # Documentation (port 3003)

# Quality
pnpm lint             # ESLint
pnpm check-types      # TypeScript
pnpm format           # Prettier

# UI
pnpm ui               # Add shadcn to @cntrl-pw/ui
```

## Project Structure

| Path           | Stack                            | Purpose                                       |
| -------------- | -------------------------------- | --------------------------------------------- |
| `apps/bridge`  | Tauri + SolidJS + Rust           | Desktop app exposing REST/WS API on port 9990 |
| `apps/hub`     | TanStack Start                   | "Bring your bridge" dashboard                 |
| `apps/rhub`    | TanStack Start                   | Raspberry Pi optimized hub                    |
| `apps/web`     | TanStack Start                   | Landing page                                  |
| `apps/docs`    | Next.js + Fumadocs               | Documentation                                 |
| `packages/sdk` | React + TanStack Query + Zustand | Bridge client, hooks, types                   |
| `packages/ui`  | React + shadcn/ui + Tailwind v4  | Shared components                             |

## Code Style

### File Naming

- All files: `kebab-case.ts` or `kebab-case.tsx`
- Examples: `bridges-provider.tsx`, `use-system-stats.ts`, `format-bytes.ts`

### Code Naming

- Components/Types: `PascalCase`
- Hooks: `useCamelCase`
- Functions/variables: `camelCase`
- Constants: `SCREAMING_SNAKE_CASE`

## SDK Usage

```typescript
// Wrap app with providers
<QueryClientProvider client={queryClient}>
  <BridgesProvider>
    <App />
  </BridgesProvider>
</QueryClientProvider>

// Manage bridges
const { bridges, addBridge, removeBridge } = useBridges();
const id = addBridge({ config: { host: '192.168.1.100', port: 9990 }, name: 'My PC' });

// Use hooks (all take bridgeId)
const { data } = useSystemStats(bridgeId);      // Real-time via WS
const { data } = useSystemInfo(bridgeId);       // Static via REST
const { data, control } = useMedia(bridgeId);   // State + mutations
const { data, kill, launch } = useProcesses(bridgeId);
const { shutdown, sleep } = usePower(bridgeId);
```

## Bridge API

Base: `http://<ip>:9990` / `ws://<ip>:9990/api/ws`

Auth: `Authorization: Bearer <api_key>` (scope-based, see below)

### Auth System

Role-based API key auth with scoped permissions. Implemented in `auth_store.rs` and `auth_scopes.rs`.

**Modes**: `public` (local-network only guard) or `protected` (API key required)

**API Keys** (`ApiKeyRecord`):

- Argon2 hashed, stored in OS keyring (`cntrl.bridge` service)
- Have scopes, expiration, revocation, created/last-used timestamps
- Token format: `ck_<64 hex chars>`, ID format: `key_<32 hex chars>`
- Source: `legacy` (migrated from config) or `user` (created in app)

**Scopes** (each key can have one or more):

| Scope               | Grants access to                                  |
| ------------------- | ------------------------------------------------- |
| `admin`             | Everything (superscope)                           |
| `system:read`       | `GET /api/status`, `GET /api/system`              |
| `usage:read`        | `GET /api/usage`                                  |
| `stats:read`        | WS stats topics (cpu, memory, gpu, disks, net)    |
| `media:read`        | `GET /api/media/status`, WS media topic           |
| `media:control`     | `POST /api/media/control`, WS media commands      |
| `processes:read`    | `GET /api/processes`, WS processes topic          |
| `processes:control` | `POST /api/processes/{kill,focus,launch}`         |
| `power:control`     | `POST /api/pw/{shutdown,restart,sleep,hibernate}` |
| `stream:read`       | `GET /api/stream`                                 |
| `ws:connect`        | `GET /api/ws`                                     |

**Middleware flow** (`auth_middleware` in `server/mod.rs`):

1. Check blocked IPs (always enforced)
2. If `public` mode → allow local network, reject external
3. Check allowed IPs (bypass auth if matched)
4. Extract Bearer token → find active record → check scope for endpoint
5. WS topics and commands are also scope-checked

### REST Endpoints

- `GET /api/status` - Health check (`system:read`)
- `GET /api/system` - Static system info (`system:read`)
- `GET /api/usage` - Dynamic usage stats (`usage:read`)
- `POST /api/media/control` - Media actions (`media:control`)
- `POST /api/processes/kill` - Kill process (`processes:control`)
- `POST /api/processes/launch` - Launch app (`processes:control`)
- `POST /api/pw/{shutdown,restart,sleep,hibernate}` - Power control (`power:control`)

### WebSocket

```json
// Subscribe
{"op": "subscribe", "data": {"topics": ["stats", "media", "processes"]}}

// Events received: system_stats, media_update, process_list

// Send commands
{"op": "media", "data": {"action": "play_pause"}}
{"op": "media", "data": {"action": "set_volume", "value": 50}}
{"op": "process_kill", "data": {"pid": 1234}}
```

## Key Patterns

### SDK Architecture

- Zustand store persists bridge configs to localStorage
- BridgesProvider manages WebSocket connections (one per bridge)
- WebSocket updates flow to React Query via `queryClient.setQueryData()`
- Hooks subscribe to WS topics with reference counting

### Widget Requirements

1. Handle loading state (skeleton)
2. Handle disconnected state (placeholder)
3. Handle errors (message + retry)
4. Responsive (mobile + desktop)

## Important Notes

- Bridge is local-network only. Never expose to internet.
- WebSocket-first for real-time data, REST for one-off requests.
- Multiple bridges supported simultaneously.
- No cloud dependency - everything runs locally.

## Type Definitions

**Source of truth**: `apps/bridge/src-tauri/src/server/types.rs`

All SDK types in `packages/sdk/src/types/api.ts` must match the Rust types exactly. Key types:

| Rust Type            | SDK Type             | Description                           |
| -------------------- | -------------------- | ------------------------------------- |
| `SystemInfo`         | `SystemInfo`         | Static system specs (GET /api/system) |
| `StreamPayload`      | `StreamPayload`      | Real-time stats (WS system_stats)     |
| `MediaStatus`        | `MediaStatus`        | Media playback state                  |
| `ProcessInfo`        | `ProcessInfo`        | Aggregated process info               |
| `ProcessListPayload` | `ProcessListPayload` | WS process_list event                 |

When updating SDK types, always verify against the Rust source.

## Changelogs

Docs changelogs live at:

- **SDK**: `apps/docs/content/docs/sdk/changelog.mdx`
- **Bridge**: `apps/docs/content/docs/(cntrl)/changelog.mdx`

Keep entries short and direct. Describe what was broken or added, not how it was fixed internally.

Good: `Fixed WebSocket subscriptions being overridden when multiple hooks subscribe to different topics.`
Bad: `Fixed by batching subscription syncs via queueMicrotask so all hooks within a single render commit accumulate their topics before we send one message with the full topic set to the bridge server which replaces subscriptions on each message.`

## Post-Plan Execution

After every plan execution, generate a changelog summarizing:

- Features added
- Breaking changes (if any)
- Files modified

Include a suggested commit message. Output the changelog in the response.
