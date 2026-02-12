import { createContext, onMount, useContext, type JSX } from "solid-js";
import { createStore } from "solid-js/store";
import type { AuthInfo } from "../lib/auth";
import * as auth from "../lib/auth";
import * as backend from "../lib/backend";
import { Config, ServerState, loadConfig } from "../lib/backend";
import { AppScreen } from "../types";

type StoreType = {
  cfg: Config | null;
  status: ServerState | null;
  auth: AuthInfo;
  page: AppScreen;
  reloadingConfig: boolean;
  loading: boolean;
};
const defaultStore: StoreType = {
  cfg: null,
  status: null,
  auth: { mode: "public", keys: [] },
  page: "main",
  reloadingConfig: false,
  loading: true,
};

const defaultStoreSetter: {
  setConfig: (cfg: Config) => void;
  setStatus: (status: ServerState) => void;
  setAuth: (info: AuthInfo) => void;
  refreshAuth: () => Promise<void>;
  setPage: (page: AppScreen) => void;
  setReloadingConfig: (reloadingConfig: boolean) => void;
  setLoading: (loading: boolean) => void;
  pollStatus: () => void;
} = {
  setConfig: () => {},
  setStatus: () => {},
  setAuth: () => {},
  refreshAuth: async () => {},
  setPage: () => {},
  setReloadingConfig: () => {},
  setLoading: () => {},
  pollStatus: () => {},
};

export const AppContext = createContext<[StoreType, typeof defaultStoreSetter]>([
  defaultStore,
  defaultStoreSetter,
]);

export const AppContextProvider = (props: { children: JSX.Element | JSX.Element[] }) => {
  const [state, setState] = createStore<StoreType>({
    cfg: null,
    status: null,
    auth: { mode: "public", keys: [] },
    page: "main",
    reloadingConfig: false,
    loading: true,
  });

  const setStatus = (status: ServerState) => {
    setState("status", status);
  };

  const pollStatus = async () => {
    const s = await backend.getServerStatus();
    setStatus(s);
  };

  const setConfig = (cfg: Config) => {
    setState("cfg", cfg);
  };

  const setAuth = (info: AuthInfo) => {
    setState("auth", info);
  };

  const refreshAuth = async () => {
    const info = await auth.getAuthInfo();
    setAuth(info);
  };

  const setLoading = (loading: boolean) => {
    setState("loading", loading);
  };

  const setReloadingConfig = (reloadingConfig: boolean) => {
    setState("reloadingConfig", reloadingConfig);
  };

  const setPage = (page: AppScreen) => {
    setState("page", page);
  };

  onMount(() => {
    loadConfig({
      onFinish: () => setLoading(false),
      onSuccess(c) {
        setConfig(c);
      },
    });

    // Enable rounded corners on macOS
    if (navigator.platform.includes("Mac")) {
      backend.enableRoundedCorners();
      backend.enableModernWindowStyle();
      // Reposition traffic lights to top-left with padding
      backend.repositionTrafficLights(12, 12);
    }

    pollStatus();
    refreshAuth();
    const interval = setInterval(pollStatus, 3000); // Poll status every 3s
    return () => clearInterval(interval);
  });

  return (
    <AppContext.Provider
      value={[
        state,
        {
          setConfig,
          setStatus,
          setAuth,
          refreshAuth,
          setPage,
          setReloadingConfig,
          setLoading,
          pollStatus,
        },
      ]}
    >
      {props.children}
    </AppContext.Provider>
  );
};

export function useApp() {
  const value = useContext(AppContext);

  if (!value) {
    throw new Error("Missing context Provider");
  }

  return value;
}
