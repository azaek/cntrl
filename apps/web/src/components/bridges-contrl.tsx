import {
  BridgeConnection,
  PingResult,
  useBridges,
  useBridgesStatus,
} from "@cntrl-pw/sdk";
import { Button } from "@cntrl-pw/ui/components/button";
import {
  AlertTriangle,
  Loader,
  Plug,
  Unlink,
  Wifi,
  WifiOff,
  Zap,
  ZapOff,
} from "lucide-react";
import CpuStat from "./cpu-stat";
import GpuStat from "./gpu-stat";
import MemoryStat from "./memory-stat";
import SystemInfo from "./system-info";

const BridgesControl = () => {
  const { bridges, addBridge } = useBridges();

  const { statuses } = useBridgesStatus(Array.from(bridges.keys()), { interval: 10000 });

  return (
    <div className="w-full max-w-sm rounded-md border p-2">
      <div className="flex w-full items-center justify-between">
        <p className="text-muted-foreground text-sm font-semibold">
          Total: {bridges.size}
        </p>
        <Button
          variant={"outline"}
          onClick={() =>
            addBridge({
              config: {
                host: "192.168.1.2",
                port: 9990,
                secure: false,
              },
              name: "Test Bridge",
            })
          }
        >
          Add Bridge
        </Button>
      </div>
      <div className="my-2 w-full border-b"></div>
      <div className="flex w-full flex-col gap-2">
        {Array.from(bridges.values()).map((bridge, index) => (
          <BridgeItem
            key={bridge.id}
            bridge={bridge}
            status={statuses?.[bridge.id]}
            idx={index}
          />
        ))}
      </div>
    </div>
  );
};

const BridgeItem = ({
  bridge,
  status,
  idx,
}: {
  bridge: BridgeConnection;
  status: PingResult | undefined;
  idx: number;
}) => {
  const { disconnect, connect } = useBridges();

  return (
    <div className="flex w-full flex-col">
      <div className="flex min-h-8 items-center justify-between">
        <div className="flex flex-col items-start">
          <p className="text-muted-foreground/50 text-sm font-medium">
            {idx + 1}.{" "}
            <span className="text-foreground text-sm font-medium">{bridge.name}</span>
          </p>
          <p className="text-muted-foreground/50 pl-3 text-xs font-medium">
            {bridge.config.host}:{bridge.config.port}
          </p>
        </div>
        <div className="flex items-center justify-end gap-3 pr-2">
          <Button
            disabled={bridge.status === "connecting"}
            variant={"outline"}
            size={"icon-sm"}
            onClick={() => {
              if (bridge.status === "connected") {
                disconnect(bridge.id);
              } else {
                connect(bridge.id);
              }
            }}
          >
            {bridge.status === "connecting" && <Loader className="animate-spin" />}
            {bridge.status === "connected" && <Unlink />}
            {bridge.status === "disconnected" && <Plug />}
            {bridge.status === "error" && <AlertTriangle className="text-red-400" />}
          </Button>
          {bridge.status === "connecting" && <Loader className="size-4 animate-spin" />}
          {bridge.status === "connected" && <Zap className="size-4 text-green-400" />}
          {bridge.status === "disconnected" && <ZapOff className="size-4 text-red-400" />}
          {status?.online ? (
            <div className="flex items-center gap-1">
              <Wifi className="size-4 text-green-400" />
              <p className="text-sm text-green-400">{status?.responseTime}ms</p>
            </div>
          ) : (
            <div className="flex items-center gap-1">
              <WifiOff className="size-4 text-red-400" />
              <p className="text-muted-foreground text-sm">Off</p>
            </div>
          )}
        </div>
      </div>
      <div className="mt-2 flex items-center gap-3">
        <SystemInfo bridgeId={bridge.id} />
      </div>
      <div className="mt-2 flex items-center gap-3">
        <CpuStat bridgeId={bridge.id} />
        <GpuStat bridgeId={bridge.id} />
        <MemoryStat bridgeId={bridge.id} />
      </div>
    </div>
  );
};

export default BridgesControl;
