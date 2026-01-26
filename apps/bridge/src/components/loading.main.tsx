import { Loader } from "lucide-solid";

const MainLoadingScreen = () => {
  return (
    <div class="text-fg-muted flex h-full w-full flex-1 items-center justify-center gap-2 text-sm">
      <Loader class="size-4 animate-spin" />
      <span class="text-sm font-medium">Loading...</span>
    </div>
  );
};

export default MainLoadingScreen;
