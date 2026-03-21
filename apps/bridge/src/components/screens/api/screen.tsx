import {
    Activity,
    ChartSpline,
    Cpu,
    Headphones,
    MonitorCog,
    Radio,
    SquareDashedMousePointer,
} from "lucide-solid";
import { useApp } from "../../../context/app-context";
import * as backend from "../../../lib/backend";
import { FeatureKey } from "../../../lib/backend";
import FeatureCard from "../../feature-card";
import { Button } from "../../ui/button";

const ApiScreen = () => {
    const [store, actions] = useApp();

    const toggleWsTopic = async (topic: backend.WebSocketTopic) => {
        const config = await backend.toggleWsTopic(topic);
        if (!config) return;
        actions.setConfig(config);
    };

    const toggle = async (feature: FeatureKey) => {
        const config = await backend.toggleFeatureWithResult(feature);
        if (!config) return;
        actions.setConfig(config);
    };

    return (
        <div class="flex w-full flex-1 flex-col gap-2.5 pt-2.5">
            <div class="w-full">
                <p class="text-secondary text-xs font-semibold uppercase">
                    Websocket Topics
                </p>
            </div>
            <FeatureCard
                icon={<Activity />}
                title="Stats Topic"
                description="Real-time system performance data"
                value={store.cfg!.websocket.stats.enabled}
                onValueChange={() => toggleWsTopic("stats")}
            />
            <FeatureCard
                icon={<Headphones />}
                title="Media Topic"
                description="Real-time media playback data"
                value={store.cfg!.websocket.media.enabled}
                onValueChange={() => toggleWsTopic("media")}
            />
            <FeatureCard
                icon={<Cpu />}
                title="Processes Topic"
                description="Real-time processes data"
                value={store.cfg!.websocket.processes.enabled}
                onValueChange={() => toggleWsTopic("processes")}
            />
            <div class="mt-3 w-full">
                <p class="text-secondary text-xs font-semibold uppercase">Rest API</p>
            </div>
            <FeatureCard
                icon={<Radio />}
                title="SSE Stream"
                description="SSE stream for real-time updates"
                value={store.cfg!.features.enable_stream}
                onValueChange={() => toggle("stream")}
            />
            <div class="w-full border-b border-dashed"></div>
            <FeatureCard
                icon={<MonitorCog />}
                title="System Info"
                description="Static system info like CPU, OS etc."
                value={store.cfg!.features.enable_system}
                onValueChange={() => toggle("system")}
            />
            <FeatureCard
                icon={<ChartSpline />}
                title="Usage Data"
                description="Real-time usage data, like CPU, RAM, etc."
                value={store.cfg!.features.enable_usage}
                onValueChange={() => toggle("usage")}
            />
            <FeatureCard
                icon={<Cpu />}
                title="Process Controls"
                description="View, kill, focus and launch processes"
                value={store.cfg!.features.enable_processes}
                onValueChange={() => toggle("processes")}
            />
            <FeatureCard
                icon={<Headphones />}
                title="Media Controls"
                description="Play, Pause, Next, Prev and Volume"
                value={store.cfg!.features.enable_media}
                onValueChange={() => toggle("media")}
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

export default ApiScreen;
