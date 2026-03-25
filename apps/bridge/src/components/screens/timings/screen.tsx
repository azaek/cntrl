import { Activity, Database, Headphones, Radio, Waves } from "lucide-solid";
import { useApp } from "../../../context/app-context";
import * as backend from "../../../lib/backend";
import IntervalCard, { type IntervalOption } from "../../interval-card";
import PostContentBlock from "../post-content-block";

const WS_STATS_OPTIONS: IntervalOption[] = [
    { value: 250, label: "250ms" },
    { value: 500, label: "500ms" },
    { value: 1000, label: "1s" },
    { value: 2000, label: "2s" },
    { value: 5000, label: "5s" },
    { value: 10000, label: "10s" },
    { value: 30000, label: "30s" },
];

const WS_MEDIA_OPTIONS: IntervalOption[] = [
    { value: 100, label: "100ms" },
    { value: 250, label: "250ms" },
    { value: 500, label: "500ms" },
    { value: 1000, label: "1s" },
    { value: 2000, label: "2s" },
    { value: 5000, label: "5s" },
];

const WS_PROCESSES_OPTIONS: IntervalOption[] = [
    { value: 1000, label: "1s" },
    { value: 2000, label: "2s" },
    { value: 3000, label: "3s" },
    { value: 5000, label: "5s" },
    { value: 10000, label: "10s" },
    { value: 30000, label: "30s" },
];

const DISK_CACHE_OPTIONS: IntervalOption[] = [
    { value: 5, label: "5s" },
    { value: 10, label: "10s" },
    { value: 30, label: "30s" },
    { value: 60, label: "1m" },
    { value: 120, label: "2m" },
    { value: 300, label: "5m" },
];

const STREAM_OPTIONS: IntervalOption[] = [
    { value: 1, label: "1s" },
    { value: 2, label: "2s" },
    { value: 5, label: "5s" },
    { value: 10, label: "10s" },
    { value: 30, label: "30s" },
    { value: 60, label: "1m" },
];

const TimingScreen = () => {
    const [store, actions] = useApp();

    const handleWsInterval = async (topic: backend.WebSocketTopic, value: number) => {
        const config = await backend.updateWsInterval(topic, value);
        if (config) actions.setConfig(config);
    };

    const handleDiskCache = async (value: number) => {
        const config = await backend.updateDiskCacheSeconds(value);
        if (config) actions.setConfig(config);
    };

    const handleStreamInterval = async (value: number) => {
        const config = await backend.updateStreamInterval(value);
        if (config) actions.setConfig(config);
    };

    return (
        <div class="flex w-full flex-1 flex-col gap-2.5 pt-2.5">
            <div class="w-full">
                <p class="text-secondary text-xs font-semibold uppercase">
                    WebSocket Intervals
                </p>
            </div>

            <IntervalCard
                icon={<Activity />}
                title="Stats"
                description="System stats push rate"
                options={WS_STATS_OPTIONS}
                value={store.cfg!.websocket.stats.interval_ms}
                onChange={(v) => handleWsInterval("stats", v)}
            />

            <IntervalCard
                icon={<Headphones />}
                title="Media"
                description="Media state check rate"
                options={WS_MEDIA_OPTIONS}
                value={store.cfg!.websocket.media.interval_ms}
                onChange={(v) => handleWsInterval("media", v)}
            />

            <IntervalCard
                icon={<Waves />}
                title="Processes"
                description="Process list refresh rate"
                options={WS_PROCESSES_OPTIONS}
                value={store.cfg!.websocket.processes.interval_ms}
                onChange={(v) => handleWsInterval("processes", v)}
            />

            <div class="w-full pt-1">
                <p class="text-secondary text-xs font-semibold uppercase">
                    Polling Intervals
                </p>
            </div>

            <IntervalCard
                icon={<Database />}
                title="Disk & GPU Cache"
                description="How long disk and GPU data is cached"
                options={DISK_CACHE_OPTIONS}
                value={store.cfg!.stats.disk_cache_seconds}
                onChange={handleDiskCache}
            />

            <IntervalCard
                icon={<Radio />}
                title="SSE Stream"
                description="REST stream endpoint poll rate"
                options={STREAM_OPTIONS}
                value={store.cfg!.stats.stream_interval_seconds}
                onChange={handleStreamInterval}
            />
            <PostContentBlock />
        </div>
    );
};

export default TimingScreen;
