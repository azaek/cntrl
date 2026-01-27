![Cover](https://github.com/azaek/cntrl/blob/main/github.png)

![License](https://img.shields.io/github/license/azaek/cntrl)
[![Release Bridge](https://github.com/azaek/cntrl/actions/workflows/release-bridge.yml/badge.svg)](https://github.com/azaek/cntrl/actions/workflows/release-bridge.yml)
![Ai-assisted](https://img.shields.io/badge/AI--assisted-262626)

The missing API for your desktop. Control power, media, and monitor stats remotely.

## Apps

| App                         | Description                                                                  |
| --------------------------- | ---------------------------------------------------------------------------- |
| **[bridge](./apps/bridge)** | Tauri desktop app for Windows/macOS - runs a local server for remote control |

## Packages

| Package                             | Description                                      |
| ----------------------------------- | ------------------------------------------------ |
| **[@cntrl-pw/sdk](./packages/sdk)** | TypeScript SDK for interacting with Cntrl Bridge |
| **[@cntrl-pw/ui](./packages/ui)**   | Shared UI components                             |

## Development

### Prerequisites

- [Node.js](https://nodejs.org/) >= 22
- [pnpm](https://pnpm.io/) 10.x
- [Rust](https://rustup.rs/) (for Bridge app)

### Setup

```bash
# Install dependencies
pnpm install

# Run all apps in dev mode
pnpm dev

# Run specific apps
pnpm dev:bridge   # Tauri desktop app
pnpm dev:hub      # Dashboard
pnpm dev:docs     # Documentation
pnpm dev:web      # Website
```

### Building

```bash
# Build all apps
pnpm build

# Build Bridge desktop app
pnpm build:bridge
```

## Bridge App

The Bridge app is a Tauri 2 desktop application that:

- Runs a local HTTP/WebSocket server for remote control
- Provides system stats, media control, and power management APIs
- Supports auto-updates with **stable** and **beta** channels

### Auto-Update Channels

Updates are delivered through GitHub Releases:

| Channel | Version Pattern | Example             |
| ------- | --------------- | ------------------- |
| Stable  | `v1.0.0`        | Regular releases    |
| Beta    | `v1.0.0-beta.1` | Pre-release testing |

The app automatically selects the update channel based on its version:

- Stable versions check `latest-stable.json`
- Beta versions check `latest-beta.json`

### Releasing

```bash
# Release stable version
git tag v1.0.0
git push origin v1.0.0

# Release beta version
git tag v1.0.0-beta.1
git push origin v1.0.0-beta.1
```

GitHub Actions will automatically build and publish to the appropriate channel.

## Tech Stack

- [Turborepo](https://turborepo.com/) + [pnpm](https://pnpm.io/) - Monorepo management
- [Tauri 2](https://tauri.app/) - Desktop app framework
- [React 19](https://react.dev/) - UI framework
- [SolidJS](https://solidjs.com/) - Bridge app frontend
- [TanStack](https://tanstack.com/) - Router, Query, Form
- [Tailwind CSS](https://tailwindcss.com/) - Styling

## License

MIT
