import {
    ChevronRight,
    Hourglass,
    Power,
    RefreshCcwDot,
    Router,
    Shield,
} from "lucide-solid";
import { useApp } from "../../../context/app-context";
import { toggleFeatureWithResult } from "../../../lib/backend";
import FeatureCardMin from "../../feature-card-min";
import { cn } from "../../utils";

const cardClass = cn(
    "w-full flex flex-col items-start rounded-md border p-3 gap-4.5 text-neutral-300",
    "hover:bg-neutral-800 transition-colors cursor-pointer",
);

const HomeScreen = () => {
    const [store, actions] = useApp();

    const toggleAutoStart = async () => {
        const res = await toggleFeatureWithResult("autostart");
        if (!res) return;
        actions.setConfig(res);
    };

    return (
        <div class="flex w-full flex-1 flex-col gap-2.5 pt-2.5">
            <div class="grid w-full grid-cols-2 gap-2.5">
                <div
                    class={cardClass}
                    role="button"
                    onClick={() => actions.setPage("auth")}
                >
                    <Shield class="size-4.5" />
                    <div class="flex flex-col items-start">
                        <p class="text-sm font-medium">Authentication</p>
                        <p class="text-xs font-medium text-neutral-500">
                            Configure <ChevronRight class="-my-1 inline-block size-3" />
                        </p>
                    </div>
                </div>
                <div class={cardClass} onClick={() => actions.setPage("api")}>
                    <Router class="size-4.5" />
                    <div class="flex flex-col items-start">
                        <p class="text-sm font-medium">API & Routes</p>
                        <p class="text-xs font-medium text-neutral-500">
                            Manage WS & more{" "}
                            <ChevronRight class="-my-1 inline-block size-3" />
                        </p>
                    </div>
                </div>
                <div class={cardClass} onClick={() => actions.setPage("timings")}>
                    <Hourglass class="size-4.5" />
                    <div class="flex flex-col items-start">
                        <p class="text-sm font-medium">Timing & Intervals</p>
                        <p class="text-xs font-medium text-neutral-500">
                            Configure <ChevronRight class="-my-1 inline-block size-3" />
                        </p>
                    </div>
                </div>
                <div class={cardClass} onClick={() => actions.setPage("power")}>
                    <Power class="size-4.5" />
                    <div class="flex flex-col items-start">
                        <p class="text-sm font-medium">Power Controls</p>
                        <p class="text-xs font-medium text-neutral-500">
                            Manage <ChevronRight class="-my-1 inline-block size-3" />
                        </p>
                    </div>
                </div>
            </div>
            <FeatureCardMin
                id="autostart"
                icon={<RefreshCcwDot class="size-4" />}
                title="Start on login"
                description="Automatically start the server when your PC starts"
                value={store.cfg!.features.enable_autostart}
                onValueChange={() => toggleAutoStart()}
            />
        </div>
    );
};

export default HomeScreen;
