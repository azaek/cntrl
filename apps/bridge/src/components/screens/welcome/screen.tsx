import { getCurrentWindow } from "@tauri-apps/api/window";
import { ArrowRight, ArrowUpRight, X } from "lucide-solid";
import { createSignal, onMount } from "solid-js";
import { useApp } from "../../../context/app-context";
import { getAppVersion } from "../../../lib/backend";
import { completeOnboarding } from "../../../lib/ui-store";
import { Logo } from "../../svgs";
import { Button } from "../../ui/button";
import { IconButton } from "../../ui/icon-btn";
import TextTip from "../../ui/text-tip";

const WelcomeScreen = () => {
    const [version, setVersion] = createSignal("");
    const [, { setPage }] = useApp();

    const handleGetStarted = async () => {
        await completeOnboarding();
        setPage("main");
    };

    const closeWindow = async () => {
        try {
            setPage("main");
            await getCurrentWindow().close();
        } catch (e) {
            console.error("Failed to close window:", e);
        }
    };

    onMount(async () => {
        const v = await getAppVersion();
        setVersion(v);
    });

    return (
        <div class="flex h-screen w-full flex-1 flex-col items-center justify-between gap-6 p-5">
            <div class="pointer-events-none z-6 flex w-full items-start justify-between">
                <div class="flex items-center gap-2">
                    <Logo className="size-8 text-neutral-300" />
                    <div class="flex flex-col items-start">
                        <p class="text-sm text-neutral-400">
                            <b class="font-semibold">Cntrl</b> Bridge
                        </p>
                        <div class="rounded border border-neutral-800 bg-neutral-950 px-1.5 py-0.5">
                            <p class="text-[11px] leading-none text-neutral-400">
                                v{version()}
                            </p>
                        </div>
                    </div>
                </div>
                <div class="pointer-events-auto">
                    <TextTip content="Close">
                        <IconButton
                            onClick={() => {
                                closeWindow();
                            }}
                            class="rounded-md border-none bg-transparent"
                        >
                            <X class="text-neutral-200" />
                        </IconButton>
                    </TextTip>
                </div>
            </div>
            <div class="flex w-full flex-1 flex-col items-start justify-between py-10">
                <div>
                    <h1 class="text-xl font-medium text-neutral-300">
                        Thanks for trying Bridge
                    </h1>
                    <p class="mt-2 text-sm text-neutral-500">
                        You're one of the first to use this — that means a lot. We're
                        still polishing things, so expect a few rough edges.
                    </p>
                </div>
                <div>
                    <p class="font-mono text-xs text-neutral-500">What's New</p>
                    <ul class="list-disc pl-4 font-mono text-sm font-medium text-neutral-600">
                        <li>Auto-reconnect support</li>
                        <li>New Auth with permissions</li>
                        <li>New User Interface</li>
                    </ul>
                </div>
            </div>
            <div class="flex w-full items-center justify-between">
                <Button class="gap-1 rounded-md px-6 text-neutral-400" variant={"ghost"}>
                    <p class="text-xs">View Changelog</p>
                    <ArrowUpRight />
                </Button>
                <Button
                    class="rounded-md px-6"
                    variant={"secondary"}
                    onClick={handleGetStarted}
                >
                    <p class="text-xs">Continue</p>
                    <ArrowRight />
                </Button>
            </div>
        </div>
    );
};

export default WelcomeScreen;
