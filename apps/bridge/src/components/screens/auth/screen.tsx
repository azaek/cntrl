import { Shield, SquareDashedMousePointer, TriangleAlert } from "lucide-solid";
import { For } from "solid-js";
import { useApp } from "../../../context/app-context";
import { getAuthInfo, setAuthMode } from "../../../lib/auth";
import { Switch } from "../../switch";
import { Button } from "../../ui/button";
import AddIpBlacklistSheet from "./add-ip-blacklist-sheet";
import AddIpWhitelistSheet from "./add-ip-whitelist-sheet";
import ApiKeyItem from "./api-key.item";
import IPItem from "./ip.item";
import NewKeySheet from "./new-key-sheet";
const AuthScreen = () => {
    const [store, actions] = useApp();

    const toggleAuth = async (active: boolean) => {
        const newAUth = active ? "protected" : "public";
        await setAuthMode(newAUth);
        const state = await getAuthInfo();
        actions.setAuth(state);
    };

    return (
        <div class="flex w-full flex-1 flex-col">
            <div class="bg-background sticky top-0 flex w-full items-center justify-between gap-1 px-2 py-2.5 text-neutral-300">
                {/* <div class="flex items-center gap-1 text-neutral-600">
                    <HouseWifi class="size-3.5 inline-block" /> <p class="text-sm font-medium">/</p>
                </div> */}
                <div class="flex items-center gap-1">
                    <Shield class="size-4" />{" "}
                    <p class="text-sm font-medium">Authentication</p>
                </div>
                <Switch
                    value={store.auth.mode === "protected"}
                    onValueChange={toggleAuth}
                />
            </div>
            <div class="flex flex-1 flex-col items-center gap-2.5">
                <div class="flex w-full gap-2 rounded-md bg-neutral-800 p-2">
                    <TriangleAlert class="size-4 text-orange-400" />
                    <p class="flex-1 text-xs text-neutral-400">
                        We strongly recommend against exposing Cntrl Bridge to public
                        networks.
                    </p>
                </div>
                <div class="flex w-full items-center justify-between px-2">
                    <p class="text-sm">API Keys</p>
                    <NewKeySheet />
                </div>
                <div class="flex w-full flex-col gap-2">
                    <For
                        each={store.auth.keys}
                        fallback={<p class="text-xs text-neutral-500">No API keys</p>}
                    >
                        {(key) => <ApiKeyItem apiKey={key} />}
                    </For>
                </div>
                <div class="w-full border-b border-dashed"></div>
                <div class="flex w-full items-center justify-between px-2">
                    <p class="text-sm">Allowed IP Addresses</p>
                    <AddIpWhitelistSheet />
                </div>
                <div class="flex w-full flex-wrap items-center gap-2 px-2">
                    <For
                        each={store.auth.allowed_ips}
                        fallback={<p class="text-xs text-neutral-500">No allowed IPs</p>}
                    >
                        {(ip) => <IPItem ip={ip} type="allowed" />}
                    </For>
                </div>
                <div class="w-full border-b border-dashed"></div>
                <div class="flex w-full items-center justify-between px-2">
                    <p class="text-sm">Blocked IP Addresses</p>
                    <AddIpBlacklistSheet />
                </div>
                <div class="flex w-full flex-wrap items-center gap-2 px-2">
                    <For
                        each={store.auth.blocked_ips}
                        fallback={<p class="text-xs text-neutral-500">No blocked IPs</p>}
                    >
                        {(ip) => <IPItem ip={ip} type="blocked" />}
                    </For>
                </div>
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
        </div>
    );
};

export default AuthScreen;
