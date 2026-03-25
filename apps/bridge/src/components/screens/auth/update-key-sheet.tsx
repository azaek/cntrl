import {
    ChartLine,
    ChevronsLeftRightEllipsis,
    Cpu,
    Headphones,
    Key,
    Monitor,
    Power,
    Radio,
    ShieldAlert,
} from "lucide-solid";
import { createMemo, createSignal, Match, Show, Switch } from "solid-js";
import { useApp } from "../../../context/app-context";
import {
    ApiKeySummary,
    getAuthInfo,
    SCOPES,
    updateApiKeyExpiration,
    updateApiKeyScopes,
} from "../../../lib/auth";
import { Button } from "../../ui/button";
import { Checkbox } from "../../ui/check-box";
import Divider from "../../ui/divider";
import {
    Drawer,
    DrawerClose,
    DrawerContent,
    DrawerDescription,
    DrawerHeader,
    DrawerTitle,
} from "../../ui/drawer";
import { Label } from "../../ui/label";
import TextTip from "../../ui/text-tip";

const UpdateKeySheet = (props: {
    open: boolean;
    onOpenChange: (open: boolean) => void;
    apiKey: ApiKeySummary;
}) => {
    return (
        <Drawer open={props.open} onOpenChange={props.onOpenChange}>
            <DrawerContent>
                <DrawerHeader class="sr-only">
                    <DrawerTitle>Update API Key</DrawerTitle>
                    <DrawerDescription>
                        Update an API key for Cntrl Bridge.
                    </DrawerDescription>
                </DrawerHeader>
                <UpdateKeySheetScreen
                    apiKey={props.apiKey}
                    onOpenChange={props.onOpenChange}
                />
            </DrawerContent>
        </Drawer>
    );
};

type ExpTime = "1d" | "2w" | "1m" | "never" | "current";

const UpdateKeySheetScreen = (props: {
    apiKey: ApiKeySummary;
    onOpenChange: (open: boolean) => void;
}) => {
    const [store, actions] = useApp();
    const [scopes, setScopes] = createSignal(props.apiKey.scopes);
    const isAdmin = createMemo(() => scopes().includes("admin"));
    const [expiration, setExpiration] = createSignal<ExpTime>("current");

    const isValid = createMemo(() => {
        return scopes().length > 0;
    });

    const handleUpdate = async () => {
        // check if expiration is updated
        if (expiration() !== "current") {
            let exp: number | undefined = undefined;
            switch (expiration()) {
                case "1d":
                    exp = 1000 * 60 * 60 * 24;
                    break;
                case "2w":
                    exp = 1000 * 60 * 60 * 24 * 14;
                    break;
                case "1m":
                    exp = 1000 * 60 * 60 * 24 * 30;
                    break;
                case "never":
                    exp = undefined;
                    break;
            }
            exp = exp ? new Date().getTime() + exp : undefined;
            await updateApiKeyExpiration(props.apiKey.id, exp || null);
        }
        if (scopes().length !== props.apiKey.scopes.length) {
            await updateApiKeyScopes(props.apiKey.id, scopes());
        }
        const state = await getAuthInfo();
        actions.setAuth(state);
        props.onOpenChange(false);
    };

    const expires = createMemo(() => {
        if (expiration() === "current") {
            if (!props.apiKey.expires_at) return "Never Expires";
            const date = new Date(props.apiKey.expires_at);
            return date.toLocaleDateString();
        }
        let exp: number | undefined = undefined;
        switch (expiration()) {
            case "1d":
                exp = 1000 * 60 * 60 * 24;
                break;
            case "2w":
                exp = 1000 * 60 * 60 * 24 * 14;
                break;
            case "1m":
                exp = 1000 * 60 * 60 * 24 * 30;
                break;
            case "never":
                exp = undefined;
                break;
        }
        exp = exp ? new Date().getTime() + exp : undefined;
        if (!exp) return "Never Expires";
        const date = new Date(exp);
        return date.toLocaleDateString();
    });

    const isExpired = createMemo(() => {
        if (expiration() === "current") {
            if (!props.apiKey.expires_at) return false;
            const date = new Date(props.apiKey.expires_at);
            return date.getTime() < new Date().getTime();
        }
        return false;
    });

    return (
        <>
            <div class="flex w-full flex-col gap-2 px-2">
                <div class="flex w-full items-center gap-2 px-2 py-3">
                    <Key class="size-4" />
                    <div class="flex flex-1 items-center justify-between">
                        <p class="text-sm font-semibold">Update {props.apiKey.name}</p>
                        <p class="text-end font-mono text-sm font-semibold">
                            ...{props.apiKey.hint}
                        </p>
                    </div>
                </div>
                <div data-corvu-no-drag class="flex flex-col">
                    <p class="text-sm font-medium text-neutral-500">Permissions</p>
                    <div class="flex w-full flex-col items-start gap-2 p-2">
                        <div
                            aria-disabled={isAdmin()}
                            class="flex w-full items-center justify-between aria-disabled:pointer-events-none aria-disabled:opacity-50"
                        >
                            <TextTip
                                content="Read static hardware information about the system."
                                side="top"
                            >
                                <p class="text-sm font-medium underline decoration-dotted">
                                    <Monitor class="mr-1 inline-block size-4" />
                                    System
                                </p>
                            </TextTip>
                            <div class="flex items-center">
                                <Checkbox
                                    id="system:read"
                                    checked={scopes().includes("system:read")}
                                    onChange={(c) => {
                                        if (c) {
                                            setScopes((prev) => [...prev, "system:read"]);
                                        } else {
                                            setScopes((prev) =>
                                                prev.filter(
                                                    (scope) => scope !== "system:read",
                                                ),
                                            );
                                        }
                                    }}
                                />
                                <Label for="system:read-input" class="pl-2">
                                    Read
                                </Label>
                            </div>
                        </div>
                        <div
                            aria-disabled={isAdmin()}
                            class="flex w-full items-center justify-between aria-disabled:pointer-events-none aria-disabled:opacity-50"
                        >
                            <TextTip
                                content="Read real-time system stats like CPU, Memory, Disk usage etc."
                                side="top"
                            >
                                <p class="text-sm font-medium underline decoration-dotted">
                                    <ChartLine class="mr-1 inline-block size-4" />
                                    Stats
                                </p>
                            </TextTip>
                            <div class="flex items-center">
                                <Checkbox
                                    id="stats:read"
                                    checked={
                                        scopes().includes("stats:read") ||
                                        scopes().includes("usage:read")
                                    }
                                    onChange={(c) => {
                                        if (c) {
                                            setScopes((prev) => [
                                                ...prev,
                                                "stats:read",
                                                "usage:read",
                                            ]);
                                        } else {
                                            setScopes((prev) =>
                                                prev.filter(
                                                    (scope) =>
                                                        scope !== "stats:read" &&
                                                        scope !== "usage:read",
                                                ),
                                            );
                                        }
                                    }}
                                />
                                <Label for="stats:read-input" class="pl-2">
                                    Read
                                </Label>
                            </div>
                        </div>
                        <div
                            aria-disabled={isAdmin()}
                            class="flex w-full items-center justify-between aria-disabled:pointer-events-none aria-disabled:opacity-50"
                        >
                            <TextTip content="Control and read device media." side="top">
                                <p class="text-sm font-medium underline decoration-dotted">
                                    <Headphones class="mr-1 inline-block size-4" />
                                    Media
                                </p>
                            </TextTip>
                            <div class="flex items-center gap-4">
                                <div class="flex items-center">
                                    <Checkbox
                                        id="media:control"
                                        checked={scopes().includes("media:control")}
                                        onChange={(c) => {
                                            if (c) {
                                                setScopes((prev) => [
                                                    ...prev,
                                                    "media:control",
                                                ]);
                                            } else {
                                                setScopes((prev) =>
                                                    prev.filter(
                                                        (scope) =>
                                                            scope !== "media:control",
                                                    ),
                                                );
                                            }
                                        }}
                                    />
                                    <Label for="media:control-input" class="pl-2">
                                        Control
                                    </Label>
                                </div>
                                <div class="flex items-center">
                                    <Checkbox
                                        id="media:read"
                                        checked={scopes().includes("media:read")}
                                        onChange={(c) => {
                                            if (c) {
                                                setScopes((prev) => [
                                                    ...prev,
                                                    "media:read",
                                                ]);
                                            } else {
                                                setScopes((prev) =>
                                                    prev.filter(
                                                        (scope) => scope !== "media:read",
                                                    ),
                                                );
                                            }
                                        }}
                                    />
                                    <Label for="media:read-input" class="pl-2">
                                        Read
                                    </Label>
                                </div>
                            </div>
                        </div>
                        <div
                            aria-disabled={isAdmin()}
                            class="flex w-full items-center justify-between aria-disabled:pointer-events-none aria-disabled:opacity-50"
                        >
                            <TextTip
                                content="Control (launch, kill, etc.) and read device processes."
                                side="top"
                            >
                                <p class="text-sm font-medium underline decoration-dotted">
                                    <Cpu class="mr-1 inline-block size-4" />
                                    Processes
                                </p>
                            </TextTip>
                            <div class="flex items-center gap-4">
                                <div class="flex items-center">
                                    <Checkbox
                                        id="processes:control"
                                        checked={scopes().includes("processes:control")}
                                        onChange={(c) => {
                                            if (c) {
                                                setScopes((prev) => [
                                                    ...prev,
                                                    "processes:control",
                                                ]);
                                            } else {
                                                setScopes((prev) =>
                                                    prev.filter(
                                                        (scope) =>
                                                            scope !== "processes:control",
                                                    ),
                                                );
                                            }
                                        }}
                                    />
                                    <Label for="processes:control-input" class="pl-2">
                                        Control
                                    </Label>
                                </div>
                                <div class="flex items-center">
                                    <Checkbox
                                        id="processes:read"
                                        checked={scopes().includes("processes:read")}
                                        onChange={(c) => {
                                            if (c) {
                                                setScopes((prev) => [
                                                    ...prev,
                                                    "processes:read",
                                                ]);
                                            } else {
                                                setScopes((prev) =>
                                                    prev.filter(
                                                        (scope) =>
                                                            scope !== "processes:read",
                                                    ),
                                                );
                                            }
                                        }}
                                    />
                                    <Label for="processes:read-input" class="pl-2">
                                        Read
                                    </Label>
                                </div>
                            </div>
                        </div>
                        <Divider />
                        <div
                            aria-disabled={isAdmin()}
                            class="flex w-full items-center justify-between aria-disabled:pointer-events-none aria-disabled:opacity-50"
                        >
                            <TextTip
                                content="Control device power like reboot, shutdown, etc."
                                side="top"
                            >
                                <p class="text-sm font-medium underline decoration-dotted">
                                    <Power class="mr-1 inline-block size-4" />
                                    Power
                                </p>
                            </TextTip>
                            <div class="flex items-center">
                                <Checkbox
                                    id="power:allow"
                                    checked={scopes().includes("power:allow")}
                                    onChange={(c) => {
                                        if (c) {
                                            setScopes((prev) => [...prev, "power:allow"]);
                                        } else {
                                            setScopes((prev) =>
                                                prev.filter(
                                                    (scope) => scope !== "power:allow",
                                                ),
                                            );
                                        }
                                    }}
                                />
                                <Label for="power:allow-input" class="pl-2">
                                    Allow
                                </Label>
                            </div>
                        </div>
                        <div
                            aria-disabled={isAdmin()}
                            class="flex w-full items-center justify-between aria-disabled:pointer-events-none aria-disabled:opacity-50"
                        >
                            <TextTip
                                content="Connect via SSE to get real-time updates."
                                side="top"
                            >
                                <p class="text-sm font-medium underline decoration-dotted">
                                    <Radio class="mr-1 inline-block size-4" />
                                    SSE Stream
                                </p>
                            </TextTip>
                            <div class="flex items-center">
                                <Checkbox
                                    id="stream:read"
                                    checked={scopes().includes("stream:read")}
                                    onChange={(c) => {
                                        if (c) {
                                            setScopes((prev) => [...prev, "stream:read"]);
                                        } else {
                                            setScopes((prev) =>
                                                prev.filter(
                                                    (scope) => scope !== "stream:read",
                                                ),
                                            );
                                        }
                                    }}
                                />
                                <Label for="stream:read-input" class="pl-2">
                                    Allow
                                </Label>
                            </div>
                        </div>
                        <div
                            aria-disabled={isAdmin()}
                            class="flex w-full items-center justify-between aria-disabled:pointer-events-none aria-disabled:opacity-50"
                        >
                            <TextTip
                                content="Connect via WebSocket to get real-time updates."
                                side="top"
                            >
                                <p class="text-sm font-medium underline decoration-dotted">
                                    <ChevronsLeftRightEllipsis class="mr-1 inline-block size-4" />
                                    WebSocket
                                </p>
                            </TextTip>
                            <div class="flex items-center">
                                <Checkbox
                                    id="ws:connect"
                                    checked={scopes().includes("ws:connect")}
                                    onChange={(c) => {
                                        if (c) {
                                            setScopes((prev) => [...prev, "ws:connect"]);
                                        } else {
                                            setScopes((prev) =>
                                                prev.filter(
                                                    (scope) => scope !== "ws:connect",
                                                ),
                                            );
                                        }
                                    }}
                                />
                                <Label for="ws:connect-input" class="pl-2">
                                    Allow
                                </Label>
                            </div>
                        </div>
                        <div class="flex w-full items-center justify-between">
                            <TextTip content="Full access to the device." side="top">
                                <p class="text-sm font-medium text-red-400 underline decoration-dotted">
                                    <ShieldAlert class="mr-1 inline-block size-4" />
                                    Full Access (Admin)
                                </p>
                            </TextTip>
                            <div class="flex items-start">
                                <Checkbox
                                    id="admin"
                                    checked={scopes().includes("admin")}
                                    onChange={(c) => {
                                        if (c) {
                                            setScopes((prev) => [...prev, "admin"]);
                                        } else {
                                            setScopes((prev) =>
                                                prev.filter((scope) => scope !== "admin"),
                                            );
                                        }
                                    }}
                                />
                                <Label for="admin-input" class="pl-2">
                                    Allow
                                </Label>
                            </div>
                        </div>
                        <Switch>
                            <Match when={isAdmin()}>
                                <p class="text-xs text-red-400">
                                    Admin access overrides all other scopes.
                                </p>
                            </Match>
                            <Match when={!isAdmin()}>
                                <p class="text-xs text-blue-400">
                                    {scopes().length}/{SCOPES.length - 2} scopes selected.
                                </p>
                            </Match>
                        </Switch>
                    </div>
                    {/* <p class="text-sm font-medium text-neutral-500">Expiration</p> */}
                    <div class="mt-2 flex w-full flex-col items-start justify-between gap-2 border-y px-2 pt-2 pb-3">
                        <div class="flex w-full items-center justify-between">
                            <TextTip
                                content="API key expiration time. Select Never for never expires."
                                side="top"
                            >
                                <p class="text-sm font-medium underline decoration-dotted">
                                    Expires in
                                </p>
                            </TextTip>
                            <div class="flex items-center justify-end text-xs">
                                <Show
                                    when={!isExpired()}
                                    fallback={<span class="text-red-400">Expired</span>}
                                >
                                    <span>{expires()}</span>
                                </Show>
                            </div>
                        </div>
                        <div class="flex w-full flex-wrap items-center gap-3">
                            <div class="flex items-start gap-2">
                                <Label for="current-input">
                                    <Checkbox
                                        id="current"
                                        checked={expiration() === "current"}
                                        onChange={(c) => {
                                            if (c) {
                                                setExpiration("current");
                                            }
                                        }}
                                    />
                                    Keep Current
                                </Label>
                            </div>
                            <div class="flex items-start gap-2">
                                <Label for="1d-input">
                                    <Checkbox
                                        id="1d"
                                        checked={expiration() === "1d"}
                                        onChange={(c) => {
                                            if (c) {
                                                setExpiration("1d");
                                            }
                                        }}
                                    />
                                    1 Day
                                </Label>
                            </div>
                            <div class="flex items-start gap-2">
                                <Label for="2w-input">
                                    <Checkbox
                                        id="2w"
                                        checked={expiration() === "2w"}
                                        onChange={(c) => {
                                            if (c) {
                                                setExpiration("2w");
                                            }
                                        }}
                                    />
                                    2 Weeks
                                </Label>
                            </div>
                            <div class="flex items-start gap-2">
                                <Label for="1m-input">
                                    <Checkbox
                                        id="1m"
                                        checked={expiration() === "1m"}
                                        onChange={(c) => {
                                            if (c) {
                                                setExpiration("1m");
                                            }
                                        }}
                                    />
                                    1 Month
                                </Label>
                            </div>
                            <div class="flex items-start gap-2">
                                <Label for="never-input">
                                    <Checkbox
                                        id="never"
                                        checked={expiration() === "never"}
                                        onChange={(c) => {
                                            if (c) {
                                                setExpiration("never");
                                            }
                                        }}
                                    />
                                    Never
                                </Label>
                            </div>
                        </div>
                    </div>
                </div>
            </div>
            <div data-corvu-no-drag class="flex w-full items-center justify-between p-2">
                <DrawerClose>
                    <Button variant="ghost">Cancel</Button>
                </DrawerClose>
                <Button disabled={!isValid()} onClick={handleUpdate} variant="secondary">
                    Update
                </Button>
            </div>
        </>
    );
};

export default UpdateKeySheet;
