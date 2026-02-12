import { Shield, TriangleAlert } from "lucide-solid";
import { useApp } from "../../../context/app-context";
import { getAuthInfo, setAuthMode } from "../../../lib/auth";
import { Switch } from "../../switch";
import ApiKeyItem from "./api-key.item";
import NewKeySheet from "./new-key-sheet";
const AuthScreen = () => {
  const [store, actions] = useApp();

  const toggleAuth = async (active: boolean) => {
    const newAUth = active ? "protected" : "public";
    await setAuthMode(newAUth);
    const state = await getAuthInfo();
    actions.setAuth(state);
  };

  return (
    <div class="flex w-full flex-1 flex-col gap-2.5 pt-2.5">
      <div class="flex w-full items-center justify-between gap-1 px-2 text-neutral-300">
        {/* <div class="flex items-center gap-1 text-neutral-600">
                    <HouseWifi class="size-3.5 inline-block" /> <p class="text-sm font-medium">/</p>
                </div> */}
        <div class="flex items-center gap-1">
          <Shield class="size-4" /> <p class="text-sm font-medium">Authentication</p>
        </div>
        <Switch value={store.auth.mode === "protected"} onValueChange={toggleAuth} />
      </div>
      <div class="flex w-full gap-2 rounded-md bg-neutral-800 p-2">
        <TriangleAlert class="size-4 text-orange-400" />
        <p class="flex-1 text-xs text-neutral-400">
          We strongly recommend against exposing Cntrl Bridge to public networks.
        </p>
      </div>
      <div class="flex w-full items-center justify-between px-2">
        <p class="text-sm">API Keys</p>
        <NewKeySheet />
      </div>
      <div class="flex w-full flex-col gap-2">
        {store.auth.keys.map((key) => (
          <ApiKeyItem apiKey={key} />
        ))}
      </div>
    </div>
  );
};

export default AuthScreen;
