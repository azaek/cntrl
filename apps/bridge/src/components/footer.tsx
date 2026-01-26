import { openUrl } from "@tauri-apps/plugin-opener";
import { BookOpen } from "lucide-solid";
import { createSignal, onMount } from "solid-js";
import { getAppVersion } from "../lib/backend";
import Divider from "./ui/divider";
const Footer = () => {
  const [version, setVersion] = createSignal("");

  onMount(async () => {
    const v = await getAppVersion();
    setVersion(v);
  });

  return (
    <>
      <Divider />
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
          {version() && <p class="text-fg-muted/50 font-mono text-xs">v{version()}</p>}
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
            onClick={() => openUrl("https://cntrl.pw")}
            title="Documentation"
          >
            <BookOpen class="size-4" />
            <p class="text-xs font-medium">Docs</p>
          </button>
        </div>
      </footer>
    </>
  );
};
export default Footer;
