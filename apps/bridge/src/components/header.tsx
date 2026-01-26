import { getCurrentWindow } from "@tauri-apps/api/window";
import { EthernetPort, X } from "lucide-solid";
import { Show } from "solid-js";
import { useApp } from "../context/app-context";
import Divider from "./ui/divider";
import { cn } from "./utils";
const Header = () => {
  const [store, actions] = useApp();

  const closeWindow = async () => {
    try {
      await getCurrentWindow().hide();
      actions.setPage("main");
    } catch (e) {
      console.error("Failed to hide window:", e);
    }
  };

  const getStatusColor = () => {
    const s = store.status?.status;
    if (!s) return "text-fg-muted";
    if (s.status === "Running") return "text-[#67FF90]";
    if (s.status === "Starting") return "text-blue-500";
    if (s.status === "Stopped") return "text-orange-500";
    if (s.status === "Error") return "text-red-500";
    return "text-fg-muted";
  };

  return (
    <>
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
        <Show when={store.status?.status.status === "Stopped"}>
          <p class="text-fg-muted text-sm font-semibold">
            Stopped{" "}
            <span class="text-fg-muted font-mono">[{store.cfg?.server.port}]</span>
          </p>
        </Show>
        <Show when={store.status?.status.status === "Error"}>
          <p class="text-sm font-semibold text-red-500">
            Error <span class="text-fg-muted font-mono">[{store.cfg?.server.port}]</span>
          </p>
        </Show>
        <Show when={store.status?.status.status === "Running"}>
          <p class="text-sm font-semibold text-[#67FF90]">
            Online <span class="text-fg-muted font-mono">[{store.cfg?.server.port}]</span>
          </p>
        </Show>
        <Show when={store.status?.status.status === "Starting"}>
          <p class="text-sm font-semibold text-blue-500">
            Starting{" "}
            <span class="text-fg-muted font-mono">[{store.cfg?.server.port}]</span>
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
      <Divider />
    </>
  );
};

export default Header;
