import { Clock3, Moon, Power, RotateCcw } from "lucide-solid";
import { useApp } from "../../../context/app-context";
import * as backend from "../../../lib/backend";
import { FeatureKey } from "../../../lib/backend";
import FeatureCard from "../../feature-card";
import PostContentBlock from "../post-content-block";

const PowerScreen = () => {
    const [store, actions] = useApp();

    const toggle = async (feature: FeatureKey) => {
        const config = await backend.toggleFeatureWithResult(feature);
        if (!config) return;
        actions.setConfig(config);
    };

    return (
        <div class="flex w-full flex-1 flex-col gap-2.5 pt-2.5">
            <div class="w-full">
                <p class="text-secondary text-xs font-semibold uppercase">
                    Power Controls
                </p>
            </div>
            <FeatureCard
                icon={<Power />}
                title="Shutdown"
                description="Allow system shutdown"
                value={store.cfg!.features.enable_shutdown}
                onValueChange={() => toggle("shutdown")}
            />
            <FeatureCard
                icon={<RotateCcw />}
                title="Restart"
                description="Allow system restart"
                value={store.cfg!.features.enable_restart}
                onValueChange={() => toggle("restart")}
            />
            <FeatureCard
                icon={<Moon />}
                title="Sleep"
                description="Allow system sleep"
                value={store.cfg!.features.enable_sleep}
                onValueChange={() => toggle("sleep")}
            />
            <FeatureCard
                icon={<Clock3 />}
                title="Hibernate"
                description="Allow system hibernate"
                value={store.cfg!.features.enable_hibernate}
                onValueChange={() => toggle("hibernate")}
            />
            <PostContentBlock />
        </div>
    );
};

export default PowerScreen;
