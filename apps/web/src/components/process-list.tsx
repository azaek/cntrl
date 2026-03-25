import { useProcesses } from "@cntrl-pw/sdk";
import { Button } from "@cntrl-pw/ui/components/button";
import { Input } from "@cntrl-pw/ui/components/input";
import { Cpu, Play, X } from "lucide-react";
import { useState } from "react";

const ProcessList = ({ bridgeId }: { bridgeId: string }) => {
    const { data, kill, launch } = useProcesses(bridgeId);
    const [launchPath, setLaunchPath] = useState("");
    const [filter, setFilter] = useState("");

    const filtered = data?.processes?.filter((p) =>
        p.name.toLowerCase().includes(filter.toLowerCase()),
    );

    return (
        <div className="flex w-full flex-col gap-2 rounded-md border p-2">
            <div className="flex items-center justify-between">
                <p className="text-muted-foreground text-xs font-semibold uppercase">
                    Processes{" "}
                    {data && (
                        <span className="text-muted-foreground/50 font-normal">
                            ({data.total_count})
                        </span>
                    )}
                </p>
            </div>

            {/* Launch */}
            <div className="flex items-center gap-1">
                <Input
                    placeholder="Path to launch..."
                    value={launchPath}
                    onChange={(e) => setLaunchPath(e.target.value)}
                    className="h-7 text-xs"
                />
                <Button
                    size="icon-sm"
                    variant="outline"
                    disabled={!launchPath}
                    onClick={() => {
                        launch.mutate({ path: launchPath });
                        setLaunchPath("");
                    }}
                >
                    <Play className="size-3" />
                </Button>
            </div>

            {/* Filter */}
            <Input
                placeholder="Filter processes..."
                value={filter}
                onChange={(e) => setFilter(e.target.value)}
                className="h-7 text-xs"
            />

            {/* List */}
            <div className="flex max-h-40 flex-col gap-0.5 overflow-y-auto">
                {filtered?.slice(0, 30).map((proc) => (
                    <div
                        key={proc.name}
                        className="hover:bg-accent flex items-center justify-between rounded px-1 py-0.5"
                    >
                        <div className="flex items-center gap-1.5">
                            <Cpu className="text-muted-foreground size-3" />
                            <span className="max-w-[140px] truncate text-xs">
                                {proc.name}
                            </span>
                            <span className="text-muted-foreground/50 text-xs">
                                x{proc.count}
                            </span>
                        </div>
                        <div className="flex items-center gap-2">
                            <span className="text-muted-foreground text-xs tabular-nums">
                                {proc.memory_mb.toFixed(0)} MB
                            </span>
                            <Button
                                size="icon-sm"
                                variant="ghost"
                                className="size-5"
                                onClick={() => kill.mutate({ name: proc.name })}
                            >
                                <X className="size-3 text-red-400" />
                            </Button>
                        </div>
                    </div>
                ))}
                {!filtered?.length && (
                    <p className="text-muted-foreground py-2 text-center text-xs">
                        {data ? "No processes match filter" : "Not connected"}
                    </p>
                )}
            </div>
        </div>
    );
};

export default ProcessList;
