import { useSystemStats } from "@cntrl-pw/sdk";
import { MemoryStick } from "lucide-react";

const MemoryStat = ({ bridgeId }: { bridgeId: string }) => {
  const { data } = useSystemStats(bridgeId, {
    connectionMode: "passive",
  });

  return (
    <div className="flex w-full flex-1 flex-col gap-1 px-1 py-1">
      <p className="text-muted-foreground/50 text-xs font-medium">RAM</p>
      <div className="flex items-center gap-1">
        <MemoryStick className="text-muted-foreground size-4" />
        <div className="bg-secondary h-2.5 w-14 rounded-full border">
          <div
            className="bg-primary h-full rounded-full transition-all duration-300"
            style={{ width: `${data?.memory?.used_percent || 0}%` }}
          ></div>
        </div>
        <p className="text-foreground text-xs font-medium tabular-nums">
          {data?.memory?.used_percent?.toFixed(0) || 0}%
        </p>
      </div>
    </div>
  );
};
export default MemoryStat;
