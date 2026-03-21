import { Clock3, Moon, Power, RotateCcw, SquareDashedMousePointer } from "lucide-solid";
import { useApp } from "../../../context/app-context";
import * as backend from "../../../lib/backend";
import { FeatureKey } from "../../../lib/backend";
import FeatureCard from "../../feature-card";
import { Button } from "../../ui/button";

const TimingScreen = () => {
    const [store, actions] = useApp();

    const toggle = async (feature: FeatureKey) => {
        const config = await backend.toggleFeatureWithResult(feature);
        if (!config) return;
        actions.setConfig(config);
    };

    return (
        <div class="flex w-full flex-1 flex-col gap-2.5 pt-2.5">
            <div class="w-full">
                <p class="text-secondary text-xs font-semibold uppercase">
                    Power Controls
                </p>
            </div>
            <FeatureCard
                icon={<Power />}
                title="Shutdown"
                description="Allow system shutdown"
                value={store.cfg!.features.enable_shutdown}
                onValueChange={() => toggle("shutdown")}
            />
            <FeatureCard
                icon={<RotateCcw />}
                title="Restart"
                description="Allow system restart"
                value={store.cfg!.features.enable_restart}
                onValueChange={() => toggle("restart")}
            />
            <FeatureCard
                icon={<Moon />}
                title="Sleep"
                description="Allow system sleep"
                value={store.cfg!.features.enable_sleep}
                onValueChange={() => toggle("sleep")}
            />
            <FeatureCard
                icon={<Clock3 />}
                title="Hibernate"
                description="Allow system hibernate"
                value={store.cfg!.features.enable_hibernate}
                onValueChange={() => toggle("hibernate")}
            />

            <div class="relative flex min-h-50 w-full flex-col items-center justify-center px-2 text-neutral-500">
                <div class="z-1 flex flex-col items-center">
                    <SquareDashedMousePointer class="size-5" />
                    <p class="mt-2 text-xs font-medium">
                        Couldn't find what you're looking for?
                    </p>
                    <div class="flex items-center gap-2">
                        <Button variant={"link"} class="text-xs">
                            Docs
                        </Button>
                        <Button variant={"link"} class="text-xs">
                            Github Issues
                        </Button>
                    </div>
                </div>
                <div
                    class="pointer-events-none absolute inset-0 z-0"
                    style={{
                        "background-image": `
                            linear-gradient(90deg, rgba(56,56,56,0.30) 1px, transparent 0),
                            linear-gradient(180deg, rgba(56,56,56,0.30) 1px, transparent 0),
                            repeating-linear-gradient(45deg, rgba(56,56,56,0.25) 0 2px, transparent 2px 6px)
                        `,
                        "background-size": "24px 24px, 24px 24px, 24px 24px",
                    }}
                />
                <div class="from-background pointer-events-none absolute inset-x-0 top-0 h-1/2 w-full bg-linear-to-b to-transparent"></div>
                <div class="from-background pointer-events-none absolute inset-x-0 bottom-0 h-1/2 w-full bg-linear-to-t to-transparent"></div>
                <div class="from-background pointer-events-none absolute inset-y-0 left-0 h-full w-[20%] bg-linear-to-r to-transparent"></div>
                <div class="from-background pointer-events-none absolute inset-y-0 right-0 h-full w-[20%] bg-linear-to-l to-transparent"></div>
            </div>
        </div>
    );
};

export default TimingScreen;
