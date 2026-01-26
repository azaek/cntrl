import { formatBytes, useSystemInfo } from "@cntrl-pw/sdk";
import { AlertTriangle, Loader, Monitor } from "lucide-react";

const SystemInfo = ({ bridgeId }: { bridgeId: string }) => {
  const { data, isLoading, error } = useSystemInfo(bridgeId);

  if (error) {
    return (
      <div className="flex h-10 w-full items-center justify-center gap-1">
        <AlertTriangle className="size-4 text-red-400" />
        <p className="text-muted-foreground text-xs font-medium">
          Failed to load system info, Bridge might be offline.
        </p>
      </div>
    );
  }
  if (isLoading) {
    return (
      <div className="flex h-10 w-full items-center justify-center">
        <Loader className="size-4 animate-spin" />
      </div>
    );
  }

  return (
    <div className="flex w-full items-start gap-2 px-3">
      <Monitor className="text-muted-foreground size-8" />
      <div className="flex w-full flex-col items-center">
        <div className="flex w-full items-center justify-between py-0.5 text-xs font-medium">
          <p className="text-muted-foreground/50">Hostname</p>
          <p className="text-muted-foreground">{data?.hostname}</p>
        </div>
        <div className="flex w-full items-center justify-between py-0.5 text-xs font-medium">
          <p className="text-muted-foreground/50">OS</p>
          <p className="text-muted-foreground">{data?.os.name}</p>
        </div>
        <div className="flex w-full items-center justify-between py-0.5 text-xs font-medium">
          <p className="text-muted-foreground/50">CPU</p>
          <p className="text-muted-foreground">{data?.cpu.brand}</p>
        </div>
        <div className="flex w-full items-center justify-between py-0.5 text-xs font-medium">
          <p className="text-muted-foreground/50">GPU</p>
          <p className="text-muted-foreground">{data?.gpu?.brand}</p>
        </div>
        <div className="flex w-full items-center justify-between py-0.5 text-xs font-medium">
          <p className="text-muted-foreground/50">Memory</p>
          <p className="text-muted-foreground">
            {data?.memory?.slots} | {formatBytes(data?.memory?.total || 0, 0)}
          </p>
        </div>
      </div>
    </div>
  );
};

export default SystemInfo;
