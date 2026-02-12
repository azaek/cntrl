import type { Component } from "solid-js";
import { Show } from "solid-js";
import Footer from "./components/footer";
import Hero from "./components/hero";
import MainLoadingScreen from "./components/loading.main";
import AuthScreen from "./components/screens/auth/screen";
import HomeScreen from "./components/screens/home/screen";
import PowerScreen from "./components/screens/power";
import SettingsScreen from "./components/screens/settings/screen";
import WsScreen from "./components/screens/ws";
import Container from "./components/ui/container";
import { AppContextProvider, useApp } from "./context/app-context";

const App: Component = () => {
  return (
    <AppContextProvider>
      <Screen />
    </AppContextProvider>
  );
};

const Screen = () => {
  const [store] = useApp();

  // createEffect(() => {
  //   const p = store.page;
  //   switch (p) {
  //     case "main":
  //       backend.setWindowSize(380, 626);
  //       break;
  //     case "auth":
  //       backend.setWindowSize(380, 532);
  //       break;
  //     case "power":
  //       backend.setWindowSize(380, 425);
  //       break;
  //     default:
  //       backend.setWindowSize(380, 425);
  //       break;
  //   }
  // });

  return (
    <>
      <div data-tauri-drag-region class="fixed z-5 h-13 w-screen" />
      <div class="flex h-screen w-screen flex-1 flex-col gap-0.5 overflow-hidden rounded-2xl bg-neutral-900 py-3 font-sans text-neutral-300 select-none">
        <Show when={!store.loading} fallback={<MainLoadingScreen />}>
          <div class="flex w-full flex-col items-center px-3">
            <Hero />
          </div>
          {/* <Header /> */}
          <Container>
            <Show when={store.page === "settings"} fallback={null}>
              <SettingsScreen />
            </Show>
            <Show when={store.page === "auth"} fallback={null}>
              <AuthScreen />
            </Show>
            <Show when={store.page === "main"} fallback={null}>
              <HomeScreen />
            </Show>
            <Show when={store.page === "power"} fallback={null}>
              <PowerScreen />
            </Show>
            <Show when={store.page === "ws"} fallback={null}>
              <WsScreen />
            </Show>
          </Container>
          <div class="flex w-full flex-col items-center px-3">
            <Footer />
          </div>
        </Show>
      </div>
    </>
  );
};

export default App;
