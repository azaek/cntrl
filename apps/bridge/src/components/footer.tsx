import { openUrl } from "@tauri-apps/plugin-opener";
import { BookOpen } from "lucide-solid";
import { createSignal, onMount } from "solid-js";
import {
  checkForUpdates,
  getAppVersion,
  installUpdate,
  UpdateProgress,
  UpdateResult,
} from "../lib/backend";
import Divider from "./ui/divider";
import { cn } from "./utils";
const Footer = () => {
  const [version, setVersion] = createSignal("");
  const [update, setUpdate] = createSignal<UpdateResult>({ status: "upToDate" });
  const [updating, setUpdating] = createSignal(false);
  const [progress, setProgress] = createSignal<UpdateProgress | null>(null);

  onMount(async () => {
    const v = await getAppVersion();
    setVersion(v);
    const update = await checkForUpdates();
    setUpdate(update);
  });

  const handleUpdate = async () => {
    if (update().status === "available" && update().update) {
      setUpdating(true);
      await installUpdate(update()?.update!, (p) => {
        setProgress(p);
      });
      setUpdating(false);
    }
  };

  return (
    <>
      <Divider />
      <footer class="z-10 flex w-full items-center justify-between gap-3 pt-2 pb-1">
        <div class="flex items-center gap-1 text-neutral-500">
          {version() && (
            <p class="text-xs tracking-tighter text-neutral-500">v{version()}</p>
          )}
          {update().status === "available" && !updating() && (
            <button
              onClick={() => {
                handleUpdate();
              }}
              class="group -my-2 flex cursor-pointer items-center gap-1 rounded-md bg-neutral-800 py-1.5 pr-3 pl-2 transition-colors hover:bg-neutral-700/50"
            >
              <UpdateIcon />
              <p class="text-[10px] font-medium text-neutral-300">Update Available!</p>
            </button>
          )}
          {updating() && (
            <div class="h-2 w-14 rounded-full bg-neutral-950">
              <div
                style={{
                  width: `${((progress()?.downloaded || 0) / (progress()?.total || 1)) * 100}%`,
                }}
                class="bg-accent h-full w-2 rounded-full transition-all duration-300 ease-in-out"
              ></div>
            </div>
          )}
        </div>
        <div class="flex items-center gap-2">
          <button
            class="hover:bg-fg-muted/10 -m-1 flex cursor-pointer items-center gap-1 rounded p-1 transition-colors"
            onClick={() => openUrl("https://github.com/azaek/cntrl")}
            title="GitHub"
          >
            <svg
              width="16"
              height="16"
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
            class="hover:bg-fg-muted/10 text-fg-muted -m-1 flex cursor-pointer items-center gap-1 rounded p-1 transition-colors"
            onClick={() => openUrl("https://cntrl.pw")}
            title="Documentation"
          >
            <BookOpen class="size-3.5" />
            <p class="text-[10px] font-medium">Docs</p>
          </button>
        </div>
      </footer>
    </>
  );
};

const UpdateIcon = (props: { class?: string }) => (
  <svg
    class={cn("aspect-square size-3.5", props.class)}
    viewBox="0 0 12 12"
    fill="none"
    xmlns="http://www.w3.org/2000/svg"
  >
    <path
      d="M7.5708 1.40918L6.5508 1.97412L6 1.65527L2.25 3.82617V3.8291L6 6V10.3448L9.75 8.17335V5.75L10.75 5.16455V8.75L6 11.5L1.25 8.75V3.25L6 0.5L7.5708 1.40918ZM9.26465 1.15967C9.35295 0.94675 9.64715 0.946745 9.73535 1.15967L9.8618 1.46533C10.0778 1.98673 10.4807 2.40309 10.9873 2.62842L11.3462 2.78808C11.5513 2.8795 11.5513 3.1781 11.3462 3.26953L10.9663 3.43847C10.4725 3.65812 10.0767 4.05972 9.85695 4.56396L9.7334 4.84668C9.6432 5.05375 9.35685 5.05375 9.2666 4.84668L9.14355 4.56396C8.9238 4.05964 8.5276 3.65814 8.0337 3.43847L7.6538 3.26953C7.4487 3.17811 7.4487 2.87949 7.6538 2.78808L8.0127 2.62842C8.51925 2.40309 8.92225 1.98674 9.1382 1.46533L9.26465 1.15967Z"
      fill="#67FF90"
    />
  </svg>
);

export default Footer;
