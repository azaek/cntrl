import { getCurrentWindow } from "@tauri-apps/api/window";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  Activity,
  BookOpen,
  ChartSpline,
  ChevronRight,
  EthernetPort,
  Headphones,
  MonitorCog,
  Power,
  Radio,
  RotateCw,
  Shield,
  SquareArrowOutUpRight,
  X,
  Zap,
} from "lucide-solid";
import type { Component } from "solid-js";
import { createEffect, createSignal, onMount, Show } from "solid-js";
import { Button } from "./components/button";
import FeatureCard from "./components/feature-card";
import { LabelBlock } from "./components/label";
import MainLoadingScreen from "./components/loading.main";
import { cn } from "./components/utils";
import * as backend from "./lib/backend";
import { Config, loadConfig } from "./lib/backend";

const App: Component = () => {
  const [cfg, setCfg] = createSignal<Config | null>(null);
  const [version] = createSignal("v0.0.6");
  const [loading, setLoading] = createSignal(true);
  const [status, setStatus] = createSignal<backend.ServerState | null>(null);
  const [reloadingConfig, setReloadingConfig] = createSignal(false);
  const [page, setPage] = createSignal<"main" | "auth" | "power">("main");

  createEffect(() => {
    const p = page();
    if (p === "main") {
      backend.setWindowSize(380, 626);
    } else {
      backend.setWindowSize(380, 270);
    }
  });

  const pollStatus = async () => {
    const s = await backend.getServerStatus();
    setStatus(s);
  };

  onMount(() => {
    loadConfig({
      onFinish: () => setLoading(false),
      onSuccess(c) {
        setCfg(c);
      },
    });

    // Enable rounded corners on macOS
    if (navigator.platform.includes("Mac")) {
      backend.enableRoundedCorners();
      backend.enableModernWindowStyle();
      // Reposition traffic lights to top-left with padding
      backend.repositionTrafficLights(12, 12);
    }

    pollStatus();
    const interval = setInterval(pollStatus, 3000); // Poll status every 3s
    return () => clearInterval(interval);
  });

  const closeWindow = async () => {
    try {
      await getCurrentWindow().hide();
    } catch (e) {
      console.error("Failed to hide window:", e);
    }
  };

  const toggle = async (feature: string) => {
    await backend.toggleFeature(feature);
    loadConfig({
      onSuccess(c) {
        setCfg(c);
      },
      onError() {},
      onFinish() {
        setLoading(false);
      },
    });
  };

  const reloadConfig = async () => {
    setReloadingConfig(true);
    await backend.reloadConfig();
    setReloadingConfig(false);
    pollStatus();
  };

  const getStatusColor = () => {
    const s = status()?.status;
    if (!s) return "text-fg-muted";
    if (s.status === "Running") return "text-[#67FF90]";
    if (s.status === "Starting") return "text-blue-500";
    if (s.status === "Stopped") return "text-orange-500";
    if (s.status === "Error") return "text-red-500";
    return "text-fg-muted";
  };

  // const getStatusText = () => {
  //   const s = status()?.status;
  //   if (!s) return "Checking...";
  //   if (s.status === 'Error') return "Error";
  //   return s.status;
  // }

  return (
    <div class="bg-bg flex min-h-screen w-screen flex-1 flex-col gap-0.5 overflow-hidden rounded-md p-1 font-sans text-white select-none">
      <Show when={!loading()} fallback={<MainLoadingScreen />}>
        {/* Header */}
        <header
          style={{
            position: "sticky",
            top: "0",
            "z-index": "10",
          }}
          data-tauri-drag-region
          class="bg-bg flex min-h-9 items-center gap-2 p-2"
        >
          <EthernetPort class={cn("size-5", getStatusColor())} stroke-width={2.2} />
          <Show when={status()?.status.status === "Stopped"}>
            <p class="text-fg-muted text-sm font-semibold">
              Stopped <span class="text-fg-muted font-mono">[{cfg()?.server.port}]</span>
            </p>
          </Show>
          <Show when={status()?.status.status === "Error"}>
            <p class="text-sm font-semibold text-red-500">
              Error <span class="text-fg-muted font-mono">[{cfg()?.server.port}]</span>
            </p>
          </Show>
          <Show when={status()?.status.status === "Running"}>
            <p class="text-sm font-semibold text-[#67FF90]">
              Online <span class="text-fg-muted font-mono">[{cfg()?.server.port}]</span>
            </p>
          </Show>
          <Show when={status()?.status.status === "Starting"}>
            <p class="text-sm font-semibold text-blue-500">
              Starting <span class="text-fg-muted font-mono">[{cfg()?.server.port}]</span>
            </p>
          </Show>
          <div class="flex-1"></div>
          <button
            onClick={closeWindow}
            class="text-fg-muted -m-1 rounded p-1 transition-colors hover:bg-white/10 hover:text-white"
          >
            <X class="size-4" />
          </button>
        </header>
        <div class="my-1 w-full border-b"></div>
        <LabelBlock>Actions</LabelBlock>
        <div class="flex w-full items-center gap-0.5">
          <Button
            onClick={() => {
              reloadConfig();
            }}
            className={cn(
              "flex-1",
              reloadingConfig() ? "cursor-not-allowed opacity-40" : "",
            )}
            // title={status()?.status.status === 'Stopped' ? "Start Service" : "Apply config changes"}
          >
            {status()?.status.status === "Stopped" ? (
              <Power />
            ) : (
              <RotateCw class={cn(reloadingConfig() ? "animate-spin" : "")} />
            )}
            <p>{status()?.status.status === "Stopped" ? "Start Service" : "Restart"}</p>
          </Button>
          <Button
            onClick={() => {
              backend.openConfig();
            }}
            className="flex-1"
          >
            <SquareArrowOutUpRight />
            <p>Config</p>
          </Button>
        </div>
        <LabelBlock>Controls</LabelBlock>
        <div class="flex w-full flex-1 flex-col gap-0.5">
          {/* Auth Card */}
          <div
            role="button"
            onClick={() => {
              setPage((p) => (p === "auth" ? "main" : "auth"));
            }}
            class="hover:bg-card/80 flex w-full items-center gap-2 rounded px-2 py-1.5"
          >
            <Shield class="size-4.5" />
            <div class="flex flex-1 flex-col items-start">
              <p class="text-sm font-medium text-white">Authentication</p>
              <p class="text-fg-muted text-xs font-medium">Mange bridge authentication</p>
            </div>
            <ChevronRight class="text-fg-muted size-4.5" />
          </div>
          <div class="hover:bg-card/80 flex w-full items-center gap-2 rounded px-2 py-1.5">
            <Power class="size-4.5" />
            <div class="flex flex-1 flex-col items-start">
              <p class="text-sm font-medium text-white">Power Controls</p>
              <p class="text-fg-muted text-xs font-medium">
                Manage remote power actions.
              </p>
            </div>
            <ChevronRight class="text-fg-muted size-4.5" />
          </div>
          {page() === "auth" && (
            <div class="hover:bg-card/80 flex w-full items-center gap-2 rounded px-2 py-1.5">
              <Shield class="size-4.5" />
              <div class="flex flex-1 flex-col items-start">
                <p class="text-sm font-medium text-white">Authentication</p>
                <p class="text-fg-muted text-xs font-medium">
                  Mange bridge authentication
                </p>
              </div>
              <ChevronRight class="text-fg-muted size-4.5" />
            </div>
          )}
          <div class="my-1 w-full border-b"></div>
          <FeatureCard
            id="system"
            icon={<MonitorCog />}
            title="System Info"
            description="Static system info like CPU, OS etc."
            value={cfg()!.features.enable_system}
            onValueChange={() => toggle("system")}
          />
          <FeatureCard
            id="usage"
            icon={<ChartSpline />}
            title="Usage Data"
            description="Real-time usage data, like CPU, RAM, etc."
            value={cfg()!.features.enable_usage}
            onValueChange={() => toggle("usage")}
          />
          <FeatureCard
            id="stream"
            icon={<Radio />}
            title="SSE Stream"
            description="Similar to Usage Data but efficient."
            value={cfg()!.features.enable_stream}
            onValueChange={() => toggle("stream")}
          />
          <FeatureCard
            id="media"
            icon={<Headphones />}
            title="Media Controls"
            description="Play, Pause, Next, Prev and Volume."
            value={cfg()!.features.enable_media}
            onValueChange={() => toggle("media")}
          />
          <FeatureCard
            id="processes"
            icon={<Activity />}
            title="Process Controls"
            description="View, kill, focus and launch processes."
            value={cfg()!.features.enable_processes}
            onValueChange={() => toggle("processes")}
          />
          <div class="my-1 w-full border-b"></div>
          <FeatureCard
            id="autostart"
            icon={<Zap />}
            title="Auto Start"
            description="Start automatically on login."
            value={cfg()!.features.enable_autostart}
            onValueChange={() => toggle("autostart")}
          />
        </div>
        <div class="my-1 w-full border-b"></div>
        <footer class="bg-bg sticky bottom-0 z-10 flex min-h-9 items-center justify-between gap-3 p-2">
          <div class="flex items-center gap-1 text-[#39393B]">
            <svg
              width="16"
              height="16"
              viewBox="0 0 16 16"
              fill="none"
              xmlns="http://www.w3.org/2000/svg"
            >
              <path
                fill-rule="evenodd"
                clip-rule="evenodd"
                d="M0 7.22581C0 3.81953 0 2.11639 1.05819 1.05819C2.11639 0 3.81953 0 7.22581 0H8.77419C12.1805 0 13.8836 0 14.9418 1.05819C16 2.11639 16 3.81953 16 7.22581V8.77419C16 12.1805 16 13.8836 14.9418 14.9418C13.8836 16 12.1805 16 8.77419 16H7.22581C3.81953 16 2.11639 16 1.05819 14.9418C0 13.8836 0 12.1805 0 8.77419V7.22581ZM10.1835 11.7626C10.1667 10.082 9.49159 8.47498 8.30315 7.28654C7.11471 6.0981 5.50768 5.42297 3.82707 5.40615C3.11448 5.39902 2.531 5.97109 2.52387 6.68368C2.51682 7.39621 3.08868 7.97957 3.80123 7.98669C4.8064 7.99675 5.76756 8.40053 6.47836 9.11133C7.18916 9.82213 7.59294 10.7833 7.60299 11.7885C7.61012 12.501 8.19348 13.0729 8.90601 13.0658C9.6186 13.0587 10.1907 12.4752 10.1835 11.7626ZM4.68584 10.9266C5.18969 11.4305 5.18972 12.2475 4.68584 12.7513C4.18195 13.2552 3.36495 13.2552 2.86104 12.7513L2.85374 12.744C2.34984 12.2401 2.34984 11.4232 2.85374 10.9193C3.35764 10.4153 4.17463 10.4153 4.67853 10.9193L4.68584 10.9266Z"
                fill="#39393B"
              />
            </svg>
            <p class="text-fg-muted/50 font-mono text-xs">{version()}</p>
          </div>
          <div class="flex items-center gap-2">
            <button
              class="hover:bg-fg-muted/10 -m-1 flex items-center gap-1 rounded p-1 transition-colors"
              onClick={() => openUrl("https://github.com/azaek/cntrl")}
              title="GitHub"
            >
              <svg
                width="18"
                height="18"
                viewBox="0 0 18 18"
                fill="none"
                xmlns="http://www.w3.org/2000/svg"
              >
                <path
                  d="M9.00002 1.5C4.85625 1.5 1.5 4.85625 1.5 9C1.5 12.3188 3.64688 15.1219 6.62813 16.1156C7.00313 16.1812 7.14375 15.9563 7.14375 15.7594C7.14375 15.5813 7.13438 14.9906 7.13438 14.3625C5.25 14.7094 4.7625 13.9031 4.6125 13.4813C4.52813 13.2656 4.1625 12.6 3.84375 12.4219C3.58125 12.2812 3.20625 11.9344 3.83438 11.925C4.425 11.9156 4.84688 12.4687 4.9875 12.6937C5.6625 13.8281 6.74063 13.5094 7.17188 13.3125C7.2375 12.825 7.43437 12.4969 7.65001 12.3094C5.98125 12.1219 4.2375 11.475 4.2375 8.60625C4.2375 7.79063 4.52813 7.11563 5.00625 6.59063C4.93125 6.40313 4.66875 5.63437 5.08125 4.60312C5.08125 4.60312 5.70937 4.40625 7.14375 5.37187C7.74376 5.20312 8.38127 5.11875 9.01877 5.11875C9.65627 5.11875 10.2938 5.20312 10.8938 5.37187C12.3281 4.39687 12.9563 4.60312 12.9563 4.60312C13.3688 5.63437 13.1063 6.40313 13.0313 6.59063C13.5094 7.11563 13.8 7.78125 13.8 8.60625C13.8 11.4844 12.0469 12.1219 10.3781 12.3094C10.65 12.5438 10.8844 12.9938 10.8844 13.6969C10.8844 14.7 10.875 15.5063 10.875 15.7594C10.875 15.9563 11.0156 16.1906 11.3906 16.1156C14.4435 15.085 16.4992 12.2222 16.5 9C16.5 4.85625 13.1438 1.5 9.00002 1.5Z"
                  fill="#79797A"
                />
              </svg>
            </button>
            <div class="h-3 border-r"></div>
            <button
              class="hover:bg-fg-muted/10 text-fg-muted -m-1 flex items-center gap-1 rounded p-1 transition-colors"
              onClick={() => openUrl("https://cntrl.azaek.xyz")}
              title="Documentation"
            >
              <BookOpen class="size-4" />
              <p class="text-xs font-medium">Docs</p>
            </button>
          </div>
        </footer>
      </Show>
    </div>
  );
};

export default App;
