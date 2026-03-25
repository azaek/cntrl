import { formatBytes, useSystemStats } from "@cntrl-pw/sdk";
import { HardDrive } from "lucide-react";

const DiskStat = ({ bridgeId }: { bridgeId: string }) => {
    const { data } = useSystemStats(bridgeId, {
        fields: ["disks"],
        connectionMode: "passive",
    });

    return (
        <div className="flex w-full flex-col gap-1 rounded-md border p-2">
            <p className="text-muted-foreground text-xs font-semibold uppercase">Disks</p>
            {data?.disks?.length ? (
                <div className="flex flex-col gap-1">
                    {data.disks.map((disk) => (
                        <div key={disk.fs} className="flex items-center gap-2">
                            <HardDrive className="text-muted-foreground size-3" />
                            <span className="text-xs font-medium">{disk.fs}</span>
                            <div className="bg-secondary h-1.5 flex-1 rounded-full">
                                <div
                                    className="bg-primary h-full rounded-full"
                                    style={{
                                        width: `${disk.used_percent}%`,
                                    }}
                                />
                            </div>
                            <span className="text-muted-foreground text-xs tabular-nums">
                                {formatBytes(disk.used)}/
                                {formatBytes(disk.used + disk.available)}
                            </span>
                        </div>
                    ))}
                </div>
            ) : (
                <p className="text-muted-foreground text-xs">Not connected</p>
            )}
        </div>
    );
};

export default DiskStat;
