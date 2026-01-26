import { ChevronLeft, CircleAlert, Clock3, Moon, Power, RotateCcw } from "lucide-solid";
import { useApp } from "../../context/app-context";
import { Feature, loadConfig, toggleFeature } from "../../lib/backend";
import FeatureCard from "../feature-card";
import { Button } from "../ui/button";
import Container from "../ui/container";
import Divider from "../ui/divider";

const PowerScreen = () => {
  const [store, actions] = useApp();

  const toggle = async (feature: Feature) => {
    await toggleFeature(feature);
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

  return (
    <>
      <Container className="max-h-80">
        <div class="bg-bg sticky top-0 z-10 flex w-full flex-col gap-0.5">
          <div class="flex w-full items-center gap-2 rounded px-2 py-1.5">
            <Button
              onClick={() => actions.setPage("main")}
              className="-mx-1 size-9! w-auto flex-[unset] items-center justify-center p-0!"
            >
              <ChevronLeft class="text-fg-muted size-4.5" />
            </Button>
            <Power class="size-4.5" />
            <div class="flex flex-1 flex-col items-start">
              <p class="text-sm font-medium text-white">Power Controls</p>
              <p class="text-fg-muted text-xs font-medium">
                Manage remote power controls
              </p>
            </div>
          </div>
          <Divider />
        </div>
        <div class="w-full px-0.5">
          <div class="bg-card flex w-full items-start gap-2 rounded p-3">
            <CircleAlert class="text-fg-muted size-4" />
            <p class="text-fg-muted flex-1 text-xs font-medium">
              By default, <b class="text-white">shutdown</b> and{" "}
              <b class="text-white">restart</b> are disabled for safety. Make sure to save
              your session before using these features.
            </p>
          </div>
        </div>
        <div class="my-4 flex w-full flex-col gap-2 pl-14">
          <FeatureCard
            icon={<Moon />}
            title="Sleep"
            description=""
            value={store.cfg!.features.enable_sleep}
            onValueChange={() => toggle("sleep")}
          />
          <FeatureCard
            icon={<Clock3 />}
            title="Hibernate"
            description=""
            value={store.cfg!.features.enable_hibernate}
            onValueChange={() => toggle("hibernate")}
          />
          <FeatureCard
            icon={<RotateCcw />}
            title="Restart"
            description=""
            value={store.cfg!.features.enable_restart}
            onValueChange={() => toggle("restart")}
          />
          <FeatureCard
            icon={<Power />}
            title="Shutdown"
            description=""
            value={store.cfg!.features.enable_shutdown}
            onValueChange={() => toggle("shutdown")}
          />
        </div>
      </Container>
    </>
  );
};

export default PowerScreen;
