# AGENTS.md

Cntrl Bridge: a monorepo for a local-only, multi-OS desktop control bridge, SDK, UI components, and consumer apps. The bridge exposes REST and WebSocket APIs on the local network; the SDK and apps consume those endpoints. The repo is organized into apps (bridge, hub, rhub, web, docs) and packages (sdk, ui) with clear conventions and shared UI components.

This file is derived from `CLAUDE.md` and a quick project scan:

- Commands: `pnpm dev*` for app targets, `pnpm lint`, `pnpm check-types`, `pnpm format`, `pnpm ui`.
- Structure: apps/bridge (Tauri + SolidJS + Rust), apps/hub/rhub/web (TanStack Start), apps/docs (Next.js + Fumadocs), packages/sdk (React + TanStack Query + Zustand), packages/ui (React + shadcn/ui + Tailwind v4).
- Code style: kebab-case filenames, PascalCase for components/types, camelCase for functions/hooks, SCREAMING_SNAKE_CASE for constants.
- SDK usage: React providers and hooks for bridges, system stats/info, media, processes, power control.
- Bridge API: REST endpoints for status/system/usage/media/process/power; WebSocket for real-time topics and commands.
- Key patterns: Zustand for bridge configs, WebSocket per bridge, WS updates flow into React Query, subscription reference counting.
- Widget requirements: loading, disconnected, error, responsive.
- Safety: bridge is local-network only; no internet exposure.
- Types: Rust types in `apps/bridge/src-tauri/src/server/types.rs` are source of truth; SDK types in `packages/sdk/src/types/api.ts` must match.
- Changelogs: docs live in `apps/docs/content/docs/sdk/changelog.mdx` and `apps/docs/content/docs/(cntrl)/changelog.mdx`.

When making changes, follow the conventions in `CLAUDE.md`, keep SDK and Rust types in sync, and after every plan execution, generate a changelog summarizing:

- Features added
- Breaking changes (if any)
- Files modified

Include a suggested commit message. Output the changelog in the response.
