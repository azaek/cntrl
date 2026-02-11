import type { Component } from "solid-js";
import { Show } from "solid-js";
import Footer from "./components/footer";
import Header from "./components/header";
import Hero from "./components/hero";
import MainLoadingScreen from "./components/loading.main";
import AuthScreen from "./components/screens/auth";
import MainScreen from "./components/screens/main";
import PowerScreen from "./components/screens/power";
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
      <div class="flex h-screen w-screen flex-1 flex-col gap-0.5 overflow-hidden rounded-2xl bg-neutral-900 p-3 font-sans text-white select-none">
        <Show when={!store.loading} fallback={<MainLoadingScreen />}>
          <Hero />
          <Header />
          <Container>
            {store.page === "auth" && <AuthScreen />}
            {store.page === "main" && <MainScreen />}
            {store.page === "power" && <PowerScreen />}
            {store.page === "ws" && <WsScreen />}
          </Container>
          <Footer />
        </Show>
      </div>
    </>
  );
};

export default App;
