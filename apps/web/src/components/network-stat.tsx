import { formatBytes, useSystemStats } from "@cntrl-pw/sdk";
import { ArrowDown, ArrowUp, Network } from "lucide-react";

const NetworkStat = ({ bridgeId }: { bridgeId: string }) => {
    const { data } = useSystemStats(bridgeId, {
        fields: ["network"],
        connectionMode: "passive",
    });

    return (
        <div className="flex w-full flex-col gap-1 rounded-md border p-2">
            <p className="text-muted-foreground text-xs font-semibold uppercase">
                Network
            </p>
            <div className="flex items-center gap-3">
                <Network className="text-muted-foreground size-4" />
                <div className="flex items-center gap-2">
                    <ArrowUp className="size-3 text-green-400" />
                    <span className="text-xs tabular-nums">
                        {formatBytes(data?.network?.bytes_sent ?? 0)}
                    </span>
                </div>
                <div className="flex items-center gap-2">
                    <ArrowDown className="size-3 text-blue-400" />
                    <span className="text-xs tabular-nums">
                        {formatBytes(data?.network?.bytes_recv ?? 0)}
                    </span>
                </div>
            </div>
        </div>
    );
};

export default NetworkStat;
