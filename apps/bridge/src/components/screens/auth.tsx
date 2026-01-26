import {
  Check,
  CheckCheck,
  ChevronLeft,
  Copy,
  Loader,
  Plus,
  RotateCw,
  Shield,
  X,
} from "lucide-solid";
import { createSignal } from "solid-js";
import { useApp } from "../../context/app-context";
import { useDebounce } from "../../hooks/use-debounce";
import {
  Config,
  addAllowedIp,
  addBlockedIp,
  generateApiKey,
  removeAllowedIp,
  removeBlockedIp,
  toggleAuth,
  updateApiKey,
} from "../../lib/backend";
import { Label } from "../label";
import { Switch } from "../switch";
import { Button } from "../ui/button";
import Container from "../ui/container";
import Divider from "../ui/divider";
import { cn } from "../utils";

const isValidIp = (ip: string) => {
  const parts = ip.split(".");
  if (parts.length !== 4) return false;
  return parts.every((part) => {
    const num = parseInt(part);
    return !isNaN(num) && num >= 0 && num <= 255;
  });
};

const AuthScreen = () => {
  const [store, actions] = useApp();

  const [apiKey, setApiKey] = createSignal(store.cfg!.auth.api_key || "");
  const [state, setState] = createSignal<"idle" | "typing" | "saving" | "saved">("idle");
  const [copied, setCopied] = createSignal(false);
  const [allowIp, setAllowIp] = createSignal("");
  const [blockIp, setBlockIp] = createSignal("");
  const [allowedIps, setAllowedIps] = createSignal(store.cfg!.auth.allowed_ips || "");
  const [blockedIps, setBlockedIps] = createSignal(store.cfg!.auth.blocked_ips || "");

  const updateFromCfg = (cfg: Config) => {
    setApiKey(cfg.auth.api_key || "");
    setAllowedIps(cfg.auth.allowed_ips || "");
    setBlockedIps(cfg.auth.blocked_ips || "");
  };

  const toggle = async () => {
    let config = await toggleAuth();
    if (config?.auth.enabled) {
      if (!config?.auth.api_key) {
        config = await generateApiKey();
      }
    } else {
      config = await updateApiKey("");
    }
    if (!config) return;
    actions.setConfig(config);
    updateFromCfg(config);
  };

  const handleCopy = () => {
    navigator.clipboard.writeText(store.cfg!.auth.api_key!);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  useDebounce(
    apiKey,
    async (value) => {
      // if (value.length == 0) return;
      if (value === store.cfg!.auth.api_key) return;
      setState("saving");
      const config = await updateApiKey(value);
      if (!config) return;
      actions.setConfig(config);
      updateFromCfg(config);
      setState("saved");
      setTimeout(() => setState("idle"), 2000);
    },
    600,
  );

  const handleAddAllowedIp = async (value: string) => {
    if (value.trim() === "" || !isValidIp(value.trim())) {
      return;
    }
    const config = await addAllowedIp(value.trim());
    if (!config) return;
    actions.setConfig(config);
    updateFromCfg(config);
    setAllowIp("");
  };

  const handleRemoveAllowedIp = async (value: string) => {
    if (value.trim() === "" || !isValidIp(value.trim())) {
      return;
    }
    const config = await removeAllowedIp(value.trim());
    if (!config) return;
    actions.setConfig(config);
    updateFromCfg(config);
    setAllowIp("");
  };

  const handleAddBlockedIp = async (value: string) => {
    if (value.trim() === "" || !isValidIp(value.trim())) {
      return;
    }
    const config = await addBlockedIp(value.trim());
    if (!config) return;
    actions.setConfig(config);
    updateFromCfg(config);
    setBlockIp("");
  };

  const handleRemoveBlockedIp = async (value: string) => {
    if (value.trim() === "" || !isValidIp(value.trim())) {
      return;
    }
    const config = await removeBlockedIp(value.trim());
    if (!config) return;
    actions.setConfig(config);
    updateFromCfg(config);
    setBlockIp("");
  };

  return (
    <>
      <Container className="max-h-106.5">
        <div class="bg-bg sticky top-0 z-10 flex w-full flex-col gap-0.5">
          <div class="flex w-full items-center gap-2 rounded px-2 py-1.5">
            <Button
              onClick={() => actions.setPage("main")}
              className="-mx-1 size-9! w-auto flex-[unset] items-center justify-center p-0!"
            >
              <ChevronLeft class="text-fg-muted size-4.5" />
            </Button>
            <Shield class="size-4.5" />
            <div class="flex flex-1 flex-col items-start">
              <p class="text-sm font-medium text-white">Authentication</p>
              <p class="text-fg-muted text-xs font-medium">Mange bridge authentication</p>
            </div>
            <Switch value={store.cfg!.auth.enabled} onValueChange={() => toggle()} />
          </div>
          <Divider />
        </div>
        <div class="my-2 flex w-full flex-col gap-1 px-2">
          <Label>API KEY</Label>
          <div class="relative flex w-full items-center">
            <input
              type="text"
              placeholder={
                store.cfg?.auth.enabled
                  ? "Enter API key"
                  : "Enable authentication to enter API key"
              }
              disabled={!store.cfg?.auth.enabled}
              value={apiKey()}
              onInput={(e) => {
                setApiKey(e.currentTarget.value);
                setState("typing");
              }}
              class={cn(
                "bg-bg-dark border-fg-muted/20 h-9 w-full rounded-md border text-sm text-white shadow-xs ring-0 transition-colors outline-none",
                "ring-fg-muted/20 flex items-center gap-2 transition-all enabled:hover:ring-1 enabled:focus:ring-1 enabled:active:ring-1",
                "pr-9 pl-3 disabled:cursor-not-allowed disabled:opacity-70",
                store.cfg?.auth.enabled ? "font-mono" : "",
              )}
            />
            {state() === "saving" && (
              <div
                class={cn(
                  "bg-bg-dark pointer-events-none absolute inset-y-0 right-9 my-auto flex size-7 items-center justify-center",
                  apiKey() ? "right-9" : "right-2",
                )}
              >
                <Loader class="size-4 animate-spin" />
              </div>
            )}
            {state() === "saved" && (
              <div
                class={cn(
                  "bg-bg-dark pointer-events-none absolute inset-y-0 right-9 my-auto flex size-7 items-center justify-center",
                  apiKey() ? "right-9" : "right-2",
                )}
              >
                <CheckCheck class="size-4 text-green-300" />
              </div>
            )}
            {store.cfg!.auth.api_key && (
              <button
                class={cn(
                  "hover:bg-fg-muted/10 flex size-7 cursor-pointer items-center justify-center rounded-md border-none outline-none",
                  "transition-colors",
                  "[&_svg:not([class*='size-'])]:size-4",
                  "absolute inset-y-0 right-2 -mx-1 my-auto p-1",
                )}
                onClick={handleCopy}
              >
                {copied() ? <Check /> : <Copy />}
              </button>
            )}
          </div>
          {store.cfg?.auth.enabled && (
            <Button
              disabled={!store.cfg?.auth.enabled}
              onClick={async () => {
                const config = await generateApiKey();
                if (!config) return;
                actions.setConfig(config);
                setApiKey(config.auth.api_key || "");
              }}
              className="text-secondary min-h-8 w-max flex-[unset] rounded-md px-3! py-1!"
            >
              <RotateCw class="size-3" />
              <p>Generate Key</p>
            </Button>
          )}
        </div>
        <Divider />
        <div class="my-2 flex w-full flex-col gap-1 px-2">
          <Label>Allowed Ip Addresses</Label>
          <div class="relative flex w-full items-center">
            <input
              type="text"
              placeholder={"123.123.123.123, [IP_ADDRESS]"}
              disabled={!store.cfg?.auth.enabled}
              value={allowIp()}
              onInput={(e) => {
                setAllowIp(e.currentTarget.value);
              }}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  if (allowIp().trim() === "" || !isValidIp(allowIp().trim())) {
                    return;
                  }
                  handleAddAllowedIp(e.currentTarget.value);
                }
              }}
              class={cn(
                "bg-bg-dark border-fg-muted/20 h-9 w-full rounded-md border text-sm text-white shadow-xs ring-0 transition-colors outline-none",
                "ring-fg-muted/20 flex items-center gap-2 transition-all enabled:hover:ring-1 enabled:focus:ring-1 enabled:active:ring-1",
                "placeholder:text-fg-muted/20 pr-9 pl-3 font-mono disabled:cursor-not-allowed disabled:opacity-70",
              )}
            />
            {
              <button
                disabled={allowIp().trim() === "" || !isValidIp(allowIp().trim())}
                class={cn(
                  "hover:bg-fg-muted/10 flex size-7 cursor-pointer items-center justify-center rounded-md border-none outline-none",
                  "transition-colors",
                  "[&_svg:not([class*='size-'])]:size-4",
                  "absolute inset-y-0 right-2 -mx-1 my-auto p-1",
                  "disabled:pointer-events-none disabled:cursor-not-allowed disabled:opacity-70",
                )}
                onClick={() => handleAddAllowedIp(allowIp().trim())}
              >
                <Plus />
              </button>
            }
          </div>
          {allowedIps().length > 0 && (
            <div class="mt-2 flex flex-wrap items-start gap-2">
              {allowedIps().map((ip) => (
                <div class="bg-card flex items-center gap-2 rounded px-2 py-1">
                  <p class="font-mono text-xs font-medium text-white">{ip}</p>
                  <button
                    class={cn(
                      "hover:bg-fg-muted/10 flex size-6 cursor-pointer items-center justify-center rounded-md border-none outline-none",
                      "transition-colors",
                      "[&_svg:not([class*='size-'])]:size-3",
                      "-mr-1 disabled:pointer-events-none disabled:cursor-not-allowed disabled:opacity-70",
                    )}
                    onClick={() => handleRemoveAllowedIp(ip)}
                  >
                    <X />
                  </button>
                </div>
              ))}
            </div>
          )}
        </div>
        <div class="mt-3 mb-2 flex w-full flex-col gap-1 px-2">
          <Label>Blocked Ip Addresses</Label>
          <div class="relative flex w-full items-center">
            <input
              type="text"
              placeholder={"123.123.123.123, [IP_ADDRESS]"}
              disabled={!store.cfg?.auth.enabled}
              value={blockIp()}
              onInput={(e) => {
                setBlockIp(e.currentTarget.value);
              }}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  if (blockIp().trim() === "" || !isValidIp(blockIp().trim())) {
                    return;
                  }
                  handleAddBlockedIp(e.currentTarget.value);
                }
              }}
              class={cn(
                "bg-bg-dark border-fg-muted/20 h-9 w-full rounded-md border text-sm text-white shadow-xs ring-0 transition-colors outline-none",
                "ring-fg-muted/20 flex items-center gap-2 transition-all enabled:hover:ring-1 enabled:focus:ring-1 enabled:active:ring-1",
                "placeholder:text-fg-muted/20 pr-9 pl-3 font-mono disabled:cursor-not-allowed disabled:opacity-70",
              )}
            />
            {
              <button
                disabled={blockIp().trim() === "" || !isValidIp(blockIp().trim())}
                class={cn(
                  "hover:bg-fg-muted/10 flex size-7 cursor-pointer items-center justify-center rounded-md border-none outline-none",
                  "transition-colors",
                  "[&_svg:not([class*='size-'])]:size-4",
                  "absolute inset-y-0 right-2 -mx-1 my-auto p-1",
                  "disabled:pointer-events-none disabled:cursor-not-allowed disabled:opacity-70",
                )}
                onClick={() => handleAddBlockedIp(blockIp().trim())}
              >
                <Plus />
              </button>
            }
          </div>
          {blockedIps().length > 0 && (
            <div class="mt-2 flex flex-wrap items-start gap-2">
              {blockedIps().map((ip) => (
                <div class="bg-card flex items-center gap-2 rounded px-2 py-1">
                  <p class="font-mono text-xs font-medium text-white">{ip}</p>
                  <button
                    class={cn(
                      "hover:bg-fg-muted/10 flex size-6 cursor-pointer items-center justify-center rounded-md border-none outline-none",
                      "transition-colors",
                      "[&_svg:not([class*='size-'])]:size-3",
                      "-mr-1 disabled:pointer-events-none disabled:cursor-not-allowed disabled:opacity-70",
                    )}
                    onClick={() => handleRemoveBlockedIp(ip)}
                  >
                    <X />
                  </button>
                </div>
              ))}
            </div>
          )}
        </div>
      </Container>
    </>
  );
};

export default AuthScreen;
