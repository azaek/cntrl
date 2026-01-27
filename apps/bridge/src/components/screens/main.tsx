import {
  Activity,
  ChartSpline,
  ChevronRight,
  Cpu,
  Headphones,
  MonitorCog,
  Power,
  Radio,
  RefreshCcw,
  Shield,
  SquareArrowOutUpRight,
  Zap,
} from "lucide-solid";
import { useApp } from "../../context/app-context";
import * as backend from "../../lib/backend";
import { Feature, loadConfig } from "../../lib/backend";
import FeatureCard from "../feature-card";
import { LabelBlock } from "../label";
import { Button } from "../ui/button";
import Container from "../ui/container";
import Divider from "../ui/divider";
import { cn } from "../utils";

const MainScreen = () => {
  const [store, actions] = useApp();

  const reloadConfig = async () => {
    actions.setReloadingConfig(true);
    await backend.reloadConfig();
    actions.pollStatus();
    actions.setReloadingConfig(false);
  };

  const toggle = async (feature: Feature) => {
    await backend.toggleFeature(feature);
    loadConfig({
      onSuccess(c) {
        actions.setConfig(c);
      },
      onError() {},
      onFinish() {
        actions.setLoading(false);
      },
    });
  };

  const toggleWsTopic = async (topic: backend.WebSocketTopic) => {
    const config = await backend.toggleWsTopic(topic);
    if (!config) return;
    actions.setConfig(config);
  };

  return (
    <>
      <Container className="max-h-130">
        <div class="flex w-full items-center gap-0.5">
          <Button
            onClick={() => {
              reloadConfig();
            }}
            disabled={store.reloadingConfig}
            className={cn(
              "flex-1",
              store.reloadingConfig ? "cursor-not-allowed opacity-40" : "",
            )}
          >
            {store.status?.status.status === "Stopped" && !store.reloadingConfig ? (
              <Power />
            ) : (
              <RefreshCcw class={cn(store.reloadingConfig ? "animate-spin" : "")} />
            )}
            <p>
              {store.status?.status.status === "Stopped" ? "Start Service" : "Restart"}
            </p>
          </Button>
          <Button
            onClick={() => {
              backend.openConfig();
            }}
            className="flex-1"
          >
            <SquareArrowOutUpRight />
            <p>Open Config</p>
          </Button>
        </div>
        <LabelBlock>Controls</LabelBlock>
        <div class="flex w-full flex-1 flex-col gap-0.5">
          {/* Auth Card */}
          <div
            role="button"
            onClick={() => {
              actions.setPage("auth");
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
          <div
            role="button"
            onClick={() => {
              actions.setPage("power");
            }}
            class="hover:bg-card/80 flex w-full items-center gap-2 rounded px-2 py-1.5"
          >
            <Power class="size-4.5" />
            <div class="flex flex-1 flex-col items-start">
              <p class="text-sm font-medium text-white">Power Controls</p>
              <p class="text-fg-muted text-xs font-medium">Manage remote power actions</p>
            </div>
            <ChevronRight class="text-fg-muted size-4.5" />
          </div>
          {/* <div
                        role="button"
                        onClick={() => {
                            actions.setPage("ws");
                        }}
                        class="hover:bg-card/80 flex w-full items-center gap-2 rounded px-2 py-1.5">
                        <ChevronsLeftRightEllipsis class="size-4.5" />
                        <div class="flex flex-1 flex-col items-start">
                            <p class="text-sm font-medium text-white">Websocket Config</p>
                            <p class="text-fg-muted text-xs font-medium">
                                Manage websocket loop intervals
                            </p>
                        </div>
                        <ChevronRight class="text-fg-muted size-4.5" />
                    </div> */}
          <Divider />
          <LabelBlock>Websocket Topics</LabelBlock>
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
            description="Real-time media data with updates"
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
          <LabelBlock>Rest Api</LabelBlock>
          <FeatureCard
            id="system"
            icon={<MonitorCog />}
            title="System Info"
            description="Static system info like CPU, OS etc."
            value={store.cfg!.features.enable_system}
            onValueChange={() => toggle("system")}
          />
          <FeatureCard
            id="stream"
            icon={<Radio />}
            title="SSE Stream"
            description="Similar to Usage Data but efficient"
            value={store.cfg!.features.enable_stream}
            onValueChange={() => toggle("stream")}
          />
          <FeatureCard
            id="usage"
            icon={<ChartSpline />}
            title="Usage Data"
            description="Real-time usage data, like CPU, RAM, etc."
            value={store.cfg!.features.enable_usage}
            onValueChange={() => toggle("usage")}
          />
          <FeatureCard
            id="media"
            icon={<Headphones />}
            title="Media Controls"
            description="Play, Pause, Next, Prev and Volume"
            value={store.cfg!.features.enable_media}
            onValueChange={() => toggle("media")}
          />
          <FeatureCard
            id="processes"
            icon={<Activity />}
            title="Process Controls"
            description="View, kill, focus and launch processes"
            value={store.cfg!.features.enable_processes}
            onValueChange={() => toggle("processes")}
          />
          <Divider />
          <FeatureCard
            id="autostart"
            icon={<Zap />}
            title="Auto Start"
            description="Start automatically on login"
            value={store.cfg!.features.enable_autostart}
            onValueChange={() => toggle("autostart")}
          />
        </div>
      </Container>
    </>
  );
};
export default MainScreen;
