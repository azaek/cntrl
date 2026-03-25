import { formatUptime, useSystemStats } from "@cntrl-pw/sdk";
import { Clock } from "lucide-react";

const UptimeStat = ({ bridgeId }: { bridgeId: string }) => {
    const { data } = useSystemStats(bridgeId, {
        connectionMode: "passive",
    });

    return (
        <div className="flex items-center gap-2 rounded-md border px-2 py-1.5">
            <Clock className="text-muted-foreground size-3.5" />
            <span className="text-muted-foreground text-xs font-medium">Uptime</span>
            <span className="text-xs font-medium tabular-nums">
                {data?.uptime ? formatUptime(data.uptime) : "--"}
            </span>
        </div>
    );
};

export default UptimeStat;
