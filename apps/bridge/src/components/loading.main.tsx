import { FileX } from "lucide-solid";
import { Show } from "solid-js";
import { useApp } from "../context/app-context";

const MainLoadingScreen = () => {
    const [store, { retryConfig }] = useApp();

    return (
        <Show
            when={!store.configError}
            fallback={
                <div class="flex h-full w-full flex-1 flex-col items-center justify-center gap-3 px-6 text-center">
                    <FileX class="text-red-400" />
                    <p class="text-sm font-semibold text-red-400">
                        Failed to load config
                    </p>
                    <p class="text-xs text-neutral-500">
                        Could not read the app configuration. Check that the config
                        directory is accessible.
                        <br />
                        <br />
                        If the error persists, please try reinstalling the app or create
                        an issue on GitHub.
                    </p>
                    <button
                        onClick={retryConfig}
                        class="mt-1 rounded-md bg-neutral-800 px-4 py-1.5 text-xs font-medium text-neutral-300 transition-colors hover:bg-neutral-700"
                    >
                        Retry
                    </button>
                </div>
            }
        >
            <div class="flex h-full w-full flex-1 items-center justify-center gap-2 text-sm">
                <svg
                    class="size-8 animate-pulse opacity-25"
                    viewBox="0 0 16 16"
                    fill="none"
                    xmlns="http://www.w3.org/2000/svg"
                >
                    <path
                        fill-rule="evenodd"
                        clip-rule="evenodd"
                        d="M0 7.22581C0 3.81953 0 2.11639 1.05819 1.05819C2.11639 0 3.81953 0 7.22581 0H8.77419C12.1805 0 13.8836 0 14.9418 1.05819C16 2.11639 16 3.81953 16 7.22581V8.77419C16 12.1805 16 13.8836 14.9418 14.9418C13.8836 16 12.1805 16 8.77419 16H7.22581C3.81953 16 2.11639 16 1.05819 14.9418C0 13.8836 0 12.1805 0 8.77419V7.22581ZM10.1835 11.7626C10.1667 10.082 9.49159 8.47498 8.30315 7.28654C7.11471 6.0981 5.50768 5.42297 3.82707 5.40615C3.11448 5.39902 2.531 5.97109 2.52387 6.68368C2.51682 7.39621 3.08868 7.97957 3.80123 7.98669C4.8064 7.99675 5.76756 8.40053 6.47836 9.11133C7.18916 9.82213 7.59294 10.7833 7.60299 11.7885C7.61012 12.501 8.19348 13.0729 8.90601 13.0658C9.6186 13.0587 10.1907 12.4752 10.1835 11.7626ZM4.68584 10.9266C5.18969 11.4305 5.18972 12.2475 4.68584 12.7513C4.18195 13.2552 3.36495 13.2552 2.86104 12.7513L2.85374 12.744C2.34984 12.2401 2.34984 11.4232 2.85374 10.9193C3.35764 10.4153 4.17463 10.4153 4.67853 10.9193L4.68584 10.9266Z"
                        fill="#a3a3a3"
                    />
                </svg>
            </div>
        </Show>
    );
};

export default MainLoadingScreen;
