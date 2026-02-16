import { getCurrentWindow } from "@tauri-apps/api/window";
import gsap from "gsap";
import { Flip } from "gsap/Flip";
import { HouseWifi, Lock, LockOpen, QrCode, X } from "lucide-solid";
import { createEffect, createSignal, onMount, Show } from "solid-js";
import { useApp } from "../context/app-context";
import { getLocalIp } from "../helper/data-helper";
import { getAppVersion, getLocalIps } from "../lib/backend";
import ServerDock from "./screens/hero/server-dock";
import { Logo } from "./svgs";
import CopyBtn from "./ui/copy-btn";
import { IconButton } from "./ui/icon-btn";
import PixelBlast from "./ui/pixel-blast";
import TextTip from "./ui/text-tip";

gsap.registerPlugin(Flip);

const Hero = () => {
    const [version, setVersion] = createSignal("");
    const [host, setHost] = createSignal("");

    let containerRef!: HTMLDivElement;
    let bannerWrapperRef!: HTMLDivElement;
    let bannerRef!: HTMLDivElement;
    let homeBtnRef!: HTMLDivElement;
    let btnGroupRef!: HTMLDivElement;
    let dockTrayRef!: HTMLDivElement;

    const [store, actions] = useApp();

    const closeWindow = async () => {
        try {
            actions.setPage("main");
            await getCurrentWindow().close();
        } catch (e) {
            console.error("Failed to close window:", e);
        }
    };

    onMount(async () => {
        const s = await getLocalIps();
        if (s) setHost(getLocalIp(s));
    });

    onMount(async () => {
        const v = await getAppVersion();
        setVersion(v);

        // Set initial state
        if (store.page === "main") {
            homeBtnRef.style.display = "none";
        } else {
            dockTrayRef.style.display = "none";
        }
    });

    // Home button enter/exit with FLIP
    createEffect(() => {
        const isMain = store.page === "main";

        // Only capture sibling buttons for reflow — exclude home button
        const siblings = Array.from(btnGroupRef.children).filter(
            (el) => el !== homeBtnRef,
        );
        const state = Flip.getState(siblings);

        // Toggle display to add/remove from flow
        const wasHidden = homeBtnRef.style.display === "none";
        homeBtnRef.style.display = isMain ? "none" : "";

        // Animate siblings reflow
        Flip.from(state, {
            duration: 0.25,
            ease: "circ.inOut",
        });

        // Animate home button enter/exit separately
        if (!isMain && wasHidden) {
            // Enter: slide in from right with blur
            gsap.fromTo(
                homeBtnRef,
                { opacity: 0, x: 20, filter: "blur(4px)" },
                {
                    opacity: 1,
                    x: 0,
                    filter: "blur(0px)",
                    duration: 0.25,
                    ease: "circ.out",
                    clearProps: "filter",
                },
            );
        } else if (isMain && !wasHidden) {
            // Exit: slide out to right with blur
            homeBtnRef.style.display = "";
            gsap.fromTo(
                homeBtnRef,
                { opacity: 1, x: 0, filter: "blur(0px)" },
                {
                    opacity: 0,
                    x: 20,
                    filter: "blur(4px)",
                    duration: 0.25,
                    ease: "circ.in",
                    onComplete: () => {
                        homeBtnRef.style.display = "none";
                    },
                },
            );
        }
    });

    // Dock Tray enter/exit — opposite of home button (visible on main, hidden otherwise)
    createEffect(() => {
        const isMain = store.page === "main";
        const wasHidden = dockTrayRef.style.display === "none";

        if (isMain && wasHidden) {
            // Enter: slide up from below
            dockTrayRef.style.display = "";
            gsap.fromTo(
                dockTrayRef,
                { opacity: 0, y: 20, filter: "blur(4px)" },
                {
                    opacity: 1,
                    y: 0,
                    filter: "blur(0px)",
                    duration: 0.25,
                    ease: "circ.out",
                    delay: 0.2,
                    clearProps: "filter",
                },
            );
        } else if (!isMain && !wasHidden) {
            // Exit: slide down and hide
            gsap.fromTo(
                dockTrayRef,
                { opacity: 1, y: 0, filter: "blur(0px)" },
                {
                    opacity: 0,
                    y: 40,
                    filter: "blur(4px)",
                    duration: 0.2,
                    ease: "circ.in",
                    onComplete: () => {
                        dockTrayRef.style.display = "none";
                    },
                },
            );
        }
    });

    // Banner/card height animation
    createEffect(() => {
        const isMain = store.page === "main";
        const tl = gsap.timeline();

        if (!isMain) {
            // Exit: banner slides up behind card, wrapper collapses
            tl.to(bannerRef, { y: "-100%", duration: 0.25, ease: "circ.inOut" })
                .to(
                    bannerWrapperRef,
                    { height: 0, marginTop: 0, duration: 0.25, ease: "circ.inOut" },
                    "<",
                )
                .to(
                    containerRef,
                    { height: 48, duration: 0.3, ease: "circ.inOut" },
                    "-=0.2",
                );
        } else {
            // Enter: card expands, then wrapper opens and banner slides down
            tl.to(containerRef, { height: 160, duration: 0.3, ease: "circ.inOut" })
                .to(
                    bannerWrapperRef,
                    { height: "auto", marginTop: "", duration: 0.25, ease: "circ.inOut" },
                    "-=0.2",
                )
                .to(bannerRef, { y: 0, duration: 0.25, ease: "circ.inOut" }, "<");
        }
    });

    return (
        <>
            <div class="pointer-events-none relative flex w-full flex-col items-center">
                <div
                    ref={containerRef}
                    class="relative z-5 flex h-40 w-full flex-col items-center justify-between overflow-hidden rounded-lg border bg-neutral-900"
                >
                    <div class="pointer-events-none z-1 flex h-11.5 w-full items-center justify-between pr-2 pl-3">
                        <div class="flex items-center gap-2">
                            <Logo className="size-5 text-neutral-600" />
                            <div class="flex flex-col gap-0.5">
                                <p class="text-[10px] leading-none font-medium text-neutral-500">
                                    <b>Cntrl</b> Bridge
                                </p>
                                <p class="text-[8px] leading-none font-bold text-neutral-600">
                                    {version()}
                                </p>
                            </div>
                        </div>
                        <div
                            ref={btnGroupRef}
                            class="pointer-events-none flex items-center justify-end gap-2"
                        >
                            {/* Home Button - animated with FLIP */}
                            <div
                                ref={homeBtnRef}
                                data-flip-id="home-btn"
                                class="overflow-hidden"
                            >
                                <TextTip content="Home">
                                    <IconButton
                                        onClick={() => {
                                            actions.setPage("main");
                                        }}
                                    >
                                        <HouseWifi class="text-neutral-500" />
                                    </IconButton>
                                </TextTip>
                            </div>
                            <TextTip content="Connect">
                                <IconButton
                                    onClick={() => {
                                        closeWindow();
                                    }}
                                >
                                    <QrCode class="text-neutral-500" />
                                </IconButton>
                            </TextTip>
                            <TextTip content="Close">
                                <IconButton
                                    onClick={() => {
                                        closeWindow();
                                    }}
                                >
                                    <X class="text-neutral-600" />
                                </IconButton>
                            </TextTip>
                        </div>
                    </div>
                    <div class="absolute inset-0 z-0 h-40 w-full">
                        <PixelBlast
                            pixelSize={2}
                            edgeFade={0}
                            patternDensity={1}
                            variant="diamond"
                            color="#262626"
                            autoPauseOffscreen={false}
                            speed={6}
                        />
                    </div>
                    {/* Dock Tray */}
                    <div
                        ref={dockTrayRef}
                        class="absolute inset-x-0 bottom-3 mx-auto flex w-max"
                    >
                        <ServerDock />
                    </div>
                </div>
                <div ref={bannerWrapperRef} class="z-0 -mt-1 w-full overflow-hidden">
                    <div
                        ref={bannerRef}
                        class="bg-border pointer-events-auto flex w-full items-center justify-between rounded-b-lg px-3 pt-2 pb-1"
                    >
                        <div class="flex items-center gap-2">
                            <Show
                                when={store.auth.mode === "protected"}
                                fallback={
                                    <TextTip content="Unprotected" side="top">
                                        <LockOpen class="size-3 text-orange-500" />
                                    </TextTip>
                                }
                            >
                                <Lock class="size-3 text-neutral-500" />
                            </Show>
                            <p class="text-xs font-medium text-neutral-600">
                                Host <span class="text-neutral-400">{host()}</span>
                            </p>
                        </div>
                        <CopyBtn
                            tip="Copy Host"
                            content={host()}
                            className="-m-2 size-auto p-2 text-neutral-500 [&_svg:not([class*='size-'])]:size-3"
                        />
                        {/* <p class="text-sm font-medium text-neutral-700">Auth <span class="text-neutral-400">{store.cfg?.auth.enabled ? "Enabled" : "Disabled"}</span></p> */}
                    </div>
                </div>
            </div>
        </>
    );
};

export default Hero;
