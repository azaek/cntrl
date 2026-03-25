import QRCode from "qrcode";
import { createEffect, createMemo, createSignal, onMount, Show } from "solid-js";
import { useApp } from "../../../context/app-context";
import { getLocalIp } from "../../../helper/data-helper";
import { getLocalIps } from "../../../lib/backend";
import { Logo } from "../../svgs";
import CopyBtn from "../../ui/copy-btn";
import { cn } from "../../utils";

const CopyField = (props: { value: string; mono?: boolean; truncate?: boolean }) => (
    <div class="flex h-9 items-center justify-between gap-2 rounded-md border border-white/5 bg-neutral-800 px-2">
        <p
            class={cn(
                "min-w-0 flex-1 text-xs text-neutral-300",
                props.mono !== false && "font-mono",
                props.truncate && "overflow-hidden text-ellipsis whitespace-nowrap",
            )}
        >
            {props.value}
        </p>
        <CopyBtn content={props.value} className="shrink-0 text-neutral-500" />
    </div>
);

const ConnectScreen = () => {
    const [store] = useApp();
    const [ip, setIp] = createSignal("");
    let canvasRef!: HTMLCanvasElement;

    const port = () => store.cfg!.server.port;
    const serverUrl = () => `http://${ip()}:${port()}`;
    const deepLinkUrl = () => `https://app.cntrl.pw/connect?host=${ip()}&port=${port()}`;

    const activeKey = createMemo(() =>
        store.auth.keys.find(
            (k) => !k.revoked_at && (!k.expires_at || k.expires_at > Date.now()),
        ),
    );

    onMount(async () => {
        const ips = await getLocalIps();
        setIp(getLocalIp(ips));
    });

    createEffect(() => {
        const currentIp = ip();
        if (!currentIp || !canvasRef) return;
        void QRCode.toCanvas(canvasRef, deepLinkUrl(), {
            width: 128,
            margin: 1,
            color: { dark: "#d4d4d4", light: "#171717" },
        });
    });

    return (
        <div class="flex w-full flex-1 flex-col gap-3 pt-2.5">
            {/* QR + Title */}
            <div class="hidden gap-3">
                <div class="relative shrink-0">
                    <canvas ref={canvasRef} class="rounded-lg" />
                    <div class="pointer-events-none absolute inset-0 flex items-center justify-center">
                        <div class="rounded-md bg-neutral-900 p-1.5 shadow-sm">
                            <Logo className="size-5 text-neutral-300" />
                        </div>
                    </div>
                </div>

                <div class="flex min-w-0 flex-1 flex-col gap-3 pt-0.5">
                    <div>
                        <p class="text-sm font-semibold text-white">
                            Add Bridge to Cntrl
                        </p>
                        <p class="text-xs text-neutral-500">
                            Scan the QR to add bridge to Cntrl App
                        </p>
                    </div>
                    <div class="flex flex-col gap-1">
                        <p class="text-[10px] font-semibold tracking-wide text-neutral-600 uppercase">
                            Or use this url
                        </p>
                        <CopyField value={deepLinkUrl()} truncate />
                    </div>
                </div>
            </div>

            {/* Divider */}
            <div class="flex items-center gap-2">
                <div class="h-px flex-1 bg-neutral-800" />
                <p class="text-xs font-semibold tracking-widest text-white uppercase">
                    Server Details
                </p>
                <div class="h-px flex-1 bg-neutral-800" />
            </div>

            {/* Server URL */}
            <div class="flex flex-col gap-1.5">
                <p class="text-xs font-semibold tracking-wide text-neutral-500 uppercase">
                    Server URL
                </p>
                <CopyField value={serverUrl()} />
            </div>

            {/* Host + Port */}
            <div class="grid grid-cols-2 gap-2">
                <div class="flex flex-col gap-1.5">
                    <p class="text-xs font-semibold tracking-wide text-neutral-500 uppercase">
                        Host
                    </p>
                    <CopyField value={ip() || "—"} />
                </div>
                <div class="flex flex-col gap-1.5">
                    <p class="text-xs font-semibold tracking-wide text-neutral-500 uppercase">
                        Port
                    </p>
                    <CopyField value={String(port())} />
                </div>
            </div>

            {/* API Key */}
            <div class="flex flex-col gap-1.5">
                <p class="text-xs font-semibold tracking-wide text-neutral-500 uppercase">
                    API Key
                </p>
                <Show
                    when={store.auth.mode === "protected" && activeKey()}
                    fallback={
                        <div class="flex h-9 items-center rounded-md border border-white/5 bg-neutral-800/50 px-3">
                            <p class="text-xs font-semibold text-neutral-600">
                                Not required (public mode)
                            </p>
                        </div>
                    }
                >
                    <CopyField
                        value={`${activeKey()!.name}  ••••${activeKey()!.hint}`}
                        mono={false}
                    />
                </Show>
            </div>
        </div>
    );
};

export default ConnectScreen;
