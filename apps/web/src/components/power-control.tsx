import { usePower } from "@cntrl-pw/sdk";
import { Button } from "@cntrl-pw/ui/components/button";
import { Moon, Power, RotateCcw } from "lucide-react";

const PowerControl = ({ bridgeId }: { bridgeId: string }) => {
    const { shutdown, restart, sleep, hibernate } = usePower(bridgeId);

    return (
        <div className="flex w-full flex-col gap-2 rounded-md border p-2">
            <p className="text-muted-foreground text-xs font-semibold uppercase">Power</p>
            <div className="flex flex-wrap items-center gap-1">
                <Button
                    size="sm"
                    variant="outline"
                    className="flex-1 text-xs"
                    onClick={() => sleep.mutate()}
                    disabled={sleep.isPending}
                >
                    <Moon className="mr-1 size-3" />
                    Sleep
                </Button>
                <Button
                    size="sm"
                    variant="outline"
                    className="flex-1 text-xs"
                    onClick={() => hibernate.mutate()}
                    disabled={hibernate.isPending}
                >
                    <Moon className="mr-1 size-3" />
                    Hibernate
                </Button>
                <Button
                    size="sm"
                    variant="outline"
                    className="flex-1 text-xs"
                    onClick={() => restart.mutate()}
                    disabled={restart.isPending}
                >
                    <RotateCcw className="mr-1 size-3" />
                    Restart
                </Button>
                <Button
                    size="sm"
                    variant="destructive"
                    className="flex-1 text-xs"
                    onClick={() => shutdown.mutate()}
                    disabled={shutdown.isPending}
                >
                    <Power className="mr-1 size-3" />
                    Shutdown
                </Button>
            </div>
        </div>
    );
};

export default PowerControl;
