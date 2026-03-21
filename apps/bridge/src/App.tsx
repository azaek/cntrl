import gsap from "gsap";
import type { Component } from "solid-js";
import { createEffect, Match, Show, Switch } from "solid-js";
import Footer from "./components/footer";
import Hero from "./components/hero";
import MainLoadingScreen from "./components/loading.main";
import ApiScreen from "./components/screens/api/screen";
import AuthScreen from "./components/screens/auth/screen";
import HomeScreen from "./components/screens/home/screen";
import PowerScreen from "./components/screens/power/screen";
import SettingsScreen from "./components/screens/settings/screen";
import TimingScreen from "./components/screens/timings/screen";
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

    let screenContainer: HTMLDivElement | undefined;

    createEffect(() => {
        if (!store.loading && screenContainer) {
            store.page; // Track page changes
            // Use requestAnimationFrame to ensure Solid has updated the DOM
            requestAnimationFrame(() => {
                gsap.fromTo(
                    screenContainer!.children,
                    {
                        opacity: 0,
                        y: 20,
                        filter: "blur(10px)",
                    },
                    {
                        opacity: 1,
                        y: 0,
                        filter: "blur(0px)",
                        duration: 0.3,
                        ease: "power2.out",
                        stagger: 0.1,
                        delay: 0.15,
                    },
                );
            });
        }
    });

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
                        <div ref={screenContainer} class="contents">
                            <Switch>
                                <Match when={store.page === "settings"}>
                                    <SettingsScreen />
                                </Match>
                                <Match when={store.page === "auth"}>
                                    <AuthScreen />
                                </Match>
                                <Match when={store.page === "api"}>
                                    <ApiScreen />
                                </Match>
                                <Match when={store.page === "main"}>
                                    <HomeScreen />
                                </Match>
                                <Match when={store.page === "power"}>
                                    <PowerScreen />
                                </Match>
                                <Match when={store.page === "timings"}>
                                    <TimingScreen />
                                </Match>
                            </Switch>
                        </div>
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
