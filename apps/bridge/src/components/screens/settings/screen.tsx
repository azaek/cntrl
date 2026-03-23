import { Cpu, Globe, Network, RefreshCcwDot, RotateCcw, Server } from "lucide-solid";
import { useApp } from "../../../context/app-context";
import * as backend from "../../../lib/backend";
import FeatureCard from "../../feature-card";
import SettingInputCard from "../../setting-input-card";
import {
    Select,
    SelectContent,
    SelectItem,
    SelectTrigger,
    SelectValue,
} from "../../ui/select";
import PostContentBlock from "../post-content-block";

type BindOption = { value: string; label: string };

const BIND_OPTIONS: BindOption[] = [
    { value: "0.0.0.0", label: "All interfaces" },
    { value: "127.0.0.1", label: "Localhost" },
];

const SettingsScreen = () => {
    const [store, actions] = useApp();

    const handlePort = async (value: string) => {
        const config = await backend.updateServerPort(parseInt(value, 10));
        if (config) actions.setConfig(config);
    };

    const handleHostname = async (value: string) => {
        const config = await backend.updateHostname(value);
        if (config) actions.setConfig(config);
    };

    const handleBindAddress = async (opt: BindOption | null) => {
        if (!opt) return;
        const config = await backend.updateServerHost(opt.value);
        if (config) actions.setConfig(config);
    };

    const handleToggle = async (feature: backend.FeatureKey) => {
        const config = await backend.toggleFeatureWithResult(feature);
        if (config) actions.setConfig(config);
    };

    const currentBind = () =>
        BIND_OPTIONS.find((o) => o.value === store.cfg!.server.host) ?? BIND_OPTIONS[0];

    return (
        <div class="flex w-full flex-1 flex-col gap-2.5 pt-2.5">
            <div class="w-full">
                <p class="text-secondary text-xs font-semibold uppercase">Server</p>
            </div>

            <div class="flex w-full gap-2 rounded-md bg-neutral-800 p-2">
                <RotateCcw class="mt-0.5 size-3.5 shrink-0 text-blue-400" />
                <p class="flex-1 text-xs text-neutral-400">
                    Port and bind address changes require a server restart to take effect.
                </p>
            </div>

            <SettingInputCard
                icon={<Server />}
                title="Port"
                description="Server port (default: 9990)"
                value={String(store.cfg!.server.port)}
                placeholder="9990"
                onSave={handlePort}
                validate={(v) => {
                    const n = parseInt(v, 10);
                    return !isNaN(n) && n >= 1024 && n <= 65535;
                }}
            />

            <div class="flex w-full items-center gap-2 rounded px-2 py-1.5 text-white [&_svg:not([class*='size-'])]:size-4.5">
                <Network />
                <div class="flex min-w-0 flex-1 flex-col items-start">
                    <p class="text-sm font-medium">Bind Address</p>
                    <p class="text-xs opacity-40">Network interface to listen on</p>
                </div>
                <Select<BindOption>
                    value={currentBind()}
                    onChange={handleBindAddress}
                    options={BIND_OPTIONS}
                    optionValue="value"
                    optionTextValue="label"
                    placement="bottom-end"
                    itemComponent={(itemProps) => (
                        <SelectItem item={itemProps.item}>
                            {itemProps.item.rawValue.label}
                        </SelectItem>
                    )}
                >
                    <SelectTrigger class="h-8 w-28 border-white/10 px-2 py-1 text-xs">
                        <SelectValue<BindOption>>
                            {(state) => state.selectedOption()?.label}
                        </SelectValue>
                    </SelectTrigger>
                    <SelectContent class="bg-background" />
                </Select>
            </div>

            <div class="w-full pt-1">
                <p class="text-secondary text-xs font-semibold uppercase">Identity</p>
            </div>

            <SettingInputCard
                icon={<Globe />}
                title="Hostname"
                description="Display name shown to SDK and Hub clients"
                value={store.cfg!.display.hostname}
                placeholder="My PC"
                onSave={handleHostname}
            />

            <div class="w-full pt-1">
                <p class="text-secondary text-xs font-semibold uppercase">System</p>
            </div>

            <FeatureCard
                icon={<Cpu />}
                title="GPU Stats"
                description="Collect GPU usage, temp, and VRAM"
                value={store.cfg!.stats.gpu_enabled}
                onValueChange={async () => {
                    const config = await backend.toggleGpuStats();
                    if (config) actions.setConfig(config);
                }}
            />

            <FeatureCard
                icon={<RefreshCcwDot />}
                title="Autostart"
                description="Launch bridge on system startup"
                value={store.cfg!.features.enable_autostart}
                onValueChange={() => handleToggle("autostart")}
            />

            <PostContentBlock />
        </div>
    );
};

export default SettingsScreen;
