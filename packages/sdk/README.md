# @cntrl-pw/sdk

SDK for [Cntrl Bridge](https://cntrl.pw) devices. REST/WebSocket client and React hooks for controlling your PC over the local network.

## Documentation

- [Getting Started](https://cntrl.pw/docs/sdk/getting-started)
- [Hooks Reference](https://cntrl.pw/docs/sdk/hooks)
- [Custom Storage](https://cntrl.pw/docs/sdk/custom-persistence)
- [Changelog](https://cntrl.pw/docs/sdk/changelog)

## Install

```bash
npm install @cntrl-pw/sdk
```

## Quick Start

```tsx
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { BridgesProvider, useBridges, useSystemStats, useMedia } from "@cntrl-pw/sdk";

const queryClient = new QueryClient();

function App() {
  return (
    <QueryClientProvider client={queryClient}>
      <BridgesProvider>
        <Dashboard />
      </BridgesProvider>
    </QueryClientProvider>
  );
}

function Dashboard() {
  const { bridges, addBridge } = useBridges();

  // Add a bridge
  const id = addBridge({ config: { host: "192.168.1.100", port: 9990 }, name: "My PC" });

  // Use hooks with bridgeId
  const { data: stats } = useSystemStats(id);
  const { data: media, control } = useMedia(id);

  return <div>{stats?.cpu_usage}% CPU</div>;
}
```

## Hooks

All hooks take a `bridgeId` as the first argument.

| Hook                  | Description                                 |
| --------------------- | ------------------------------------------- |
| `useSystemStats(id)`  | Real-time CPU, RAM, GPU stats via WebSocket |
| `useSystemInfo(id)`   | Static system info (OS, hardware) via REST  |
| `useMedia(id)`        | Media playback state + controls             |
| `useProcesses(id)`    | Running processes + kill/launch             |
| `usePower(id)`        | Shutdown, restart, sleep, hibernate         |
| `useBridgeStatus(id)` | Connection status                           |

## Low-Level Client

Use `@cntrl-pw/sdk/client` for direct API access without React:

```ts
import { createBridgeClient } from "@cntrl-pw/sdk/client";

const client = createBridgeClient({ host: "192.168.1.100", port: 9990 });
const info = await client.getSystemInfo();
await client.mediaControl("play_pause");
```

## Types

Import types separately:

```ts
import type { SystemInfo, MediaStatus, ProcessInfo } from "@cntrl-pw/sdk/types";
```

## License

MIT
