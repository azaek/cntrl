import { Check, X } from "lucide-solid";
import { createSignal, Show } from "solid-js";
import { useApp } from "../../../context/app-context";
import { getAuthInfo, removeAllowedIp, removeBlockedIp } from "../../../lib/auth";

const IPItem = (props: { ip: string; type: "allowed" | "blocked" }) => {
    const [confirm, setConfirm] = createSignal(false);
    const [_, action] = useApp();
    let timerRef!: number;
    const handleRemove = async () => {
        if (!confirm()) {
            clearTimeout(timerRef);
            setConfirm(true);
            timerRef = setTimeout(() => setConfirm(false), 2000);
        } else {
            clearTimeout(timerRef);
            if (props.type === "allowed") {
                await removeAllowedIp(props.ip);
            } else {
                await removeBlockedIp(props.ip);
            }
            const stats = await getAuthInfo();
            action.setAuth(stats);
        }
    };

    return (
        <div class="bg-card flex h-6 items-stretch rounded-md px-2">
            <div class="relative flex min-w-16 items-center border-r pr-2">
                <p class="font-mono text-xs break-all">{props.ip}</p>
                <Show when={confirm()}>
                    <div class="bg-card absolute inset-0 right-0 flex h-full items-center justify-center">
                        <p class="text-xs">Remove?</p>
                    </div>
                </Show>
            </div>
            <button onClick={() => handleRemove()} class="cursor-pointer pl-1">
                <Show when={!confirm()} fallback={<Check class="size-3.5" />}>
                    <X class="size-3.5" />
                </Show>
            </button>
        </div>
    );
};

export default IPItem;
