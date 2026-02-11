import { getCurrentWindow } from "@tauri-apps/api/window";
import gsap from "gsap";
import { X } from "lucide-solid";
import { createEffect, createSignal, onMount } from "solid-js";
import { useApp } from "../context/app-context";
import { getAppVersion } from "../lib/backend";
import { Logo } from "./svgs";
import { IconButton } from "./ui/icon-btn";
import PixelBlast from "./ui/pixel-blast";

const Hero = () => {
  const [version, setVersion] = createSignal("");

  let containerRef!: HTMLDivElement;
  let bannerWrapperRef!: HTMLDivElement;
  let bannerRef!: HTMLDivElement;

  const [store, actions] = useApp();

  const closeWindow = async () => {
    try {
      await getCurrentWindow().hide();
      actions.setPage("main");
    } catch (e) {
      console.error("Failed to hide window:", e);
    }
  };

  onMount(async () => {
    const v = await getAppVersion();
    setVersion(v);
  });

  createEffect(() => {
    const isAuth = store.page === "auth";
    const tl = gsap.timeline();

    if (isAuth) {
      // Exit: banner slides up behind card, wrapper collapses
      tl.to(bannerRef, { y: "-100%", duration: 0.25, ease: "circ.inOut" })
        .to(
          bannerWrapperRef,
          { height: 0, marginTop: 0, duration: 0.25, ease: "circ.inOut" },
          "<",
        )
        .to(containerRef, { height: 48, duration: 0.3, ease: "circ.inOut" }, "-=0.2");
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
      <div class="relative flex w-full flex-col items-center">
        <div
          ref={containerRef}
          class="relative z-5 flex h-40 w-full flex-col overflow-hidden rounded-lg border bg-neutral-900"
        >
          <div class="z-1 flex h-12 w-full items-center justify-between px-3">
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
            <div class="flex items-center justify-end gap-2">
              <IconButton
                onClick={() => {
                  closeWindow();
                }}
              >
                <X class="text-fg-muted size-4.5" />
              </IconButton>
            </div>
          </div>
          <div class="absolute inset-0 z-0 h-40 w-full">
            <PixelBlast
              pixelSize={3}
              edgeFade={0}
              patternDensity={1.2}
              variant="diamond"
              color="#262626"
              autoPauseOffscreen={false}
              speed={6}
            />
          </div>
        </div>
        <div ref={bannerWrapperRef} class="z-0 -mt-1 w-full overflow-hidden">
          <div
            ref={bannerRef}
            class="bg-border flex w-full items-center justify-between rounded-b-lg px-3 pt-2 pb-1"
          >
            <p class="text-sm font-medium text-white">Bridge</p>
          </div>
        </div>
      </div>
    </>
  );
};

export default Hero;
