import type { Component } from "solid-js";
import { createEffect, Show } from "solid-js";
import Footer from "./components/footer";
import Header from "./components/header";
import MainLoadingScreen from "./components/loading.main";
import AuthScreen from "./components/screens/auth";
import MainScreen from "./components/screens/main";
import PowerScreen from "./components/screens/power";
import WsScreen from "./components/screens/ws";
import { AppContextProvider, useApp } from "./context/app-context";
import * as backend from "./lib/backend";

const App: Component = () => {
  return (
    <AppContextProvider>
      <Screen />
    </AppContextProvider>
  );
};

const Screen = () => {
  const [store] = useApp();

  createEffect(() => {
    const p = store.page;
    switch (p) {
      case "main":
        backend.setWindowSize(380, 626);
        break;
      case "auth":
        backend.setWindowSize(380, 532);
        break;
      case "power":
        backend.setWindowSize(380, 425);
        break;
      default:
        backend.setWindowSize(380, 425);
        break;
    }
  });

  return (
    <div class="bg-bg flex min-h-screen w-screen flex-1 flex-col gap-0.5 overflow-hidden rounded-md p-1 font-sans text-white select-none">
      <Show when={!store.loading} fallback={<MainLoadingScreen />}>
        {/* Header */}
        <Header />
        {store.page === "auth" && <AuthScreen />}
        {store.page === "main" && <MainScreen />}
        {store.page === "power" && <PowerScreen />}
        {store.page === "ws" && <WsScreen />}
        <Footer />
      </Show>
    </div>
  );
};

export default App;
