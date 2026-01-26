import { useSystemStats } from "@cntrl-pw/sdk";
import { Cpu } from "lucide-react";

const CpuStat = ({ bridgeId }: { bridgeId: string }) => {
  const { data } = useSystemStats(bridgeId, {
    connectionMode: "eager",
  });

  return (
    <div className="flex w-full flex-1 flex-col gap-1 px-1 py-1">
      <p className="text-muted-foreground/50 text-xs font-medium">CPU</p>
      <div className="flex items-center gap-1">
        <Cpu className="text-muted-foreground size-4" />
        <div className="bg-secondary h-2.5 w-14 rounded-full border">
          <div
            className="bg-primary h-full rounded-full transition-all duration-300"
            style={{ width: `${data?.cpu?.current_load || 0}%` }}
          ></div>
        </div>
        <p className="text-foreground text-xs font-medium tabular-nums">
          {data?.cpu?.current_load?.toFixed(0) || 0}%
        </p>
      </div>
    </div>
  );
};
export default CpuStat;
