import {
  CircleStop,
  EthernetPort,
  Loader,
  PlayCircle,
  RefreshCw,
  RefreshCwOff,
  Settings2,
  TriangleAlert,
} from "lucide-solid";
import { createSignal, Show } from "solid-js";
import { useApp } from "../../../context/app-context";
import * as backend from "../../../lib/backend";
import { IconButtonGhost } from "../../ui/icon-btn";
import { Tooltip, TooltipContent, TooltipTrigger } from "../../ui/tooltip";
import { cn } from "../../utils";

const ServerDock = () => {
  const [stoppingServer, setStoppingServer] = createSignal(false);
  const [startingServer, setStartingServer] = createSignal(false);
  const [refreshingServer, setRefreshingServer] = createSignal(false);
  const [store, actions] = useApp();

  const stopServer = async () => {
    setStoppingServer(true);
    const s = await backend.stopServer();
    if (s) actions.setStatus(s);
    setStoppingServer(false);
  };

  const startServer = async () => {
    setStartingServer(true);
    const s = await backend.startServer();
    if (s) actions.setStatus(s);
    setStartingServer(false);
  };

  const refreshServer = async () => {
    setRefreshingServer(true);
    const s = await backend.restartServer();
    if (s) actions.setStatus(s);
    setRefreshingServer(false);
  };

  return (
    <div class="pointer-events-auto flex h-9 w-max items-center gap-3 overflow-hidden rounded-lg border bg-neutral-950/25 pr-px pl-3 backdrop-blur-sm">
      <div class="flex items-center gap-2">
        <div
          class={cn(
            "absolute inset-y-0 left-0 my-auto h-4 w-2 translate-x-[-25%] blur-sm",
            store.status?.status.status === "Error" && "bg-red-500",
            store.status?.status.status === "Running" && "bg-accent",
            store.status?.status.status === "Stopped" && "bg-neutral-500",
            !store.status && "bg-neutral-500",
          )}
        />
        <Show
          when={store.status?.status.status !== "Error"}
          fallback={
            <>
              <TriangleAlert class="size-4 text-red-500" />
              <Tooltip
                placement="top"
                openDelay={1000}
                closeDelay={100}
                skipDelayDuration={500}
              >
                <TooltipTrigger>
                  <p class="cursor-pointer text-xs font-medium text-red-500 underline decoration-dotted">
                    Error
                  </p>
                </TooltipTrigger>
                <TooltipContent class="max-w-3xs">
                  Error : {(store.status?.status as any).message}
                </TooltipContent>
              </Tooltip>
            </>
          }
        >
          <Show
            when={store.status?.status.status === "Stopped"}
            fallback={
              <EthernetPort
                class={cn(
                  "size-4",
                  store.status?.status.status === "Error" && "text-red-500",
                  store.status?.status.status === "Running" && "text-accent",
                  store.status?.status.status === "Stopped" && "text-neutral-500",
                  !store.status && "text-neutral-500",
                )}
              />
            }
          >
            <svg
              class="text-neutral-500"
              width="16"
              height="16"
              viewBox="0 0 16 16"
              fill="none"
              xmlns="http://www.w3.org/2000/svg"
            >
              <path
                d="M14.6681 3.99862V9.99862C14.6681 10.3522 14.5276 10.6914 14.2775 10.9414C14.0275 11.1915 13.6883 11.332 13.3347 11.332H12.0014L10.0014 13.332H6.00138M12.1381 2.66528L2.66805 2.66528C2.31443 2.66528 1.97529 2.80576 1.72524 3.05581C1.47519 3.30586 1.33472 3.64499 1.33472 3.99862V9.99862C1.33472 10.3522 1.47519 10.6914 1.72524 10.9414C1.97529 11.1915 2.31443 11.332 2.66805 11.332H3.5852"
                stroke="currentColor"
                stroke-linecap="round"
                stroke-linejoin="round"
              />
              <path
                d="M4 5.33472V6.00138"
                stroke="currentColor"
                stroke-linecap="round"
                stroke-linejoin="round"
              />
              <path
                d="M6.66528 5.33472V6.00138"
                stroke="currentColor"
                stroke-linecap="round"
                stroke-linejoin="round"
              />
              <path
                d="M13.9567 1.0625L1.25049 13.7687"
                stroke="currentColor"
                stroke-linecap="round"
              />
            </svg>
          </Show>
          <p class="text-xs font-medium text-neutral-500">
            {store.status?.status.status === "Error" && "Error"}
            {store.status?.status.status === "Running" && (store.status?.port || "9990")}
            {store.status?.status.status === "Stopped" && "Offline"}
            {!store.status && "Unknown"}
          </p>
        </Show>
      </div>
      <div class="h-3 border-r border-neutral-700" />
      <div class="flex items-center justify-end">
        <Show when={store.status?.status.status === "Running"} fallback={null}>
          <Tooltip
            placement="top"
            openDelay={1000}
            closeDelay={100}
            skipDelayDuration={500}
          >
            <TooltipTrigger
              as={IconButtonGhost}
              onClick={stopServer}
              className="border-none text-neutral-500"
            >
              <Show when={stoppingServer()} fallback={<CircleStop class="size-3.5" />}>
                <Loader class="size-3.5 animate-spin" />
              </Show>
            </TooltipTrigger>
            <TooltipContent>Stop server</TooltipContent>
          </Tooltip>
        </Show>
        <Show
          when={
            store.status?.status.status === "Stopped" ||
            store.status?.status.status === "Error"
          }
          fallback={null}
        >
          <Tooltip
            placement="top"
            openDelay={1000}
            closeDelay={100}
            skipDelayDuration={500}
          >
            <TooltipTrigger
              as={IconButtonGhost}
              onClick={startServer}
              className="border-none text-neutral-500"
            >
              <Show
                when={startingServer()}
                fallback={<PlayCircle class="text-accent size-3.5" />}
              >
                <Loader class="size-3.5 animate-spin" />
              </Show>
            </TooltipTrigger>
            <TooltipContent>Start server</TooltipContent>
          </Tooltip>
        </Show>
        <Tooltip
          placement="top"
          openDelay={1000}
          closeDelay={100}
          skipDelayDuration={500}
        >
          <TooltipTrigger
            as={IconButtonGhost}
            onClick={refreshServer}
            disabled={
              refreshingServer() ||
              store.status?.status.status === "Stopped" ||
              store.status?.status.status === "Error"
            }
            className="border-none text-neutral-500"
          >
            <Show
              when={
                refreshingServer() ||
                store.status?.status.status === "Stopped" ||
                store.status?.status.status === "Error"
              }
              fallback={
                <Show when={refreshingServer()} fallback={<RefreshCw class="size-3.5" />}>
                  <Loader class="size-3.5 animate-spin" />
                </Show>
              }
            >
              <RefreshCwOff class="size-3.5" />
            </Show>
          </TooltipTrigger>
          <TooltipContent>
            <Show
              when={
                refreshingServer() ||
                store.status?.status.status === "Stopped" ||
                store.status?.status.status === "Error"
              }
            >
              Restart disabled
            </Show>
            <Show
              when={
                !refreshingServer() &&
                store.status?.status.status !== "Stopped" &&
                store.status?.status.status !== "Error"
              }
            >
              Restart server
            </Show>
          </TooltipContent>
        </Tooltip>
        <Tooltip
          placement="top"
          openDelay={1000}
          closeDelay={100}
          skipDelayDuration={500}
        >
          <TooltipTrigger
            as={IconButtonGhost}
            onClick={() => {
              actions.setPage("settings");
            }}
            className="border-none text-neutral-500"
          >
            <Settings2 class="size-3.5" />
          </TooltipTrigger>
          <TooltipContent>Server settings</TooltipContent>
        </Tooltip>
      </div>
    </div>
  );
};

export default ServerDock;
