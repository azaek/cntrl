import { createSignal, onCleanup, onMount, type JSX } from "solid-js";

type ScrollAreaProps = {
  children: JSX.Element;
  class?: string;
  style?: JSX.CSSProperties;
  /** Delay (ms) before the scrollbar fades out after scrolling stops. Default 800 */
  scrollHideDelay?: number;
  /** Scrollbar thumb color. Default "rgba(255,255,255,0.35)" */
  thumbColor?: string;
  /** Scrollbar track width in px. Default 6 */
  trackWidth?: number;
};

export default function ScrollArea(props: ScrollAreaProps) {
  let viewportEl!: HTMLDivElement;
  let trackEl!: HTMLDivElement;
  let thumbEl!: HTMLDivElement;

  const [thumbHeight, setThumbHeight] = createSignal(0);
  const [thumbTop, setThumbTop] = createSignal(0);
  const [visible, setVisible] = createSignal(false);
  const [dragging, setDragging] = createSignal(false);

  const hideDelay = () => props.scrollHideDelay ?? 800;

  let hideTimer: ReturnType<typeof setTimeout> | undefined;
  let dragStartY = 0;
  let dragStartScrollTop = 0;

  // ── compute thumb size & position ──────────────────────────────────
  const updateThumb = () => {
    const el = viewportEl;
    if (!el) return;
    const ratio = el.clientHeight / el.scrollHeight;
    // hide thumb if content fits
    if (ratio >= 1) {
      setThumbHeight(0);
      return;
    }
    const trackH = el.clientHeight;
    const tH = Math.max(ratio * trackH, 12); // min 24px thumb
    const scrollFraction = el.scrollTop / (el.scrollHeight - el.clientHeight);
    const maxTop = trackH - tH;
    setThumbHeight(tH);
    setThumbTop(scrollFraction * maxTop);
  };

  const showBar = () => {
    clearTimeout(hideTimer);
    setVisible(true);
    hideTimer = setTimeout(() => {
      if (!dragging()) setVisible(false);
    }, hideDelay());
  };

  // ── scroll listener ────────────────────────────────────────────────
  const onScroll = () => {
    updateThumb();
    showBar();
  };

  // ── thumb drag ─────────────────────────────────────────────────────
  const onThumbPointerDown = (e: PointerEvent) => {
    e.preventDefault();
    e.stopPropagation();
    setDragging(true);
    dragStartY = e.clientY;
    dragStartScrollTop = viewportEl.scrollTop;
    thumbEl.setPointerCapture(e.pointerId);
  };

  const onThumbPointerMove = (e: PointerEvent) => {
    if (!dragging()) return;
    const el = viewportEl;
    const trackH = el.clientHeight;
    const ratio = el.clientHeight / el.scrollHeight;
    const tH = Math.max(ratio * trackH, 24);
    const maxTop = trackH - tH;
    const maxScroll = el.scrollHeight - el.clientHeight;
    const dy = e.clientY - dragStartY;
    const scrollDelta = (dy / maxTop) * maxScroll;
    el.scrollTop = dragStartScrollTop + scrollDelta;
  };

  const onThumbPointerUp = () => {
    setDragging(false);
    hideTimer = setTimeout(() => setVisible(false), hideDelay());
  };

  // ── track click to jump ────────────────────────────────────────────
  const onTrackClick = (e: MouseEvent) => {
    if (e.target === thumbEl) return;
    const rect = trackEl.getBoundingClientRect();
    const clickRatio = (e.clientY - rect.top) / rect.height;
    const el = viewportEl;
    el.scrollTop = clickRatio * (el.scrollHeight - el.clientHeight);
  };

  // ── resize observer to recompute thumb ─────────────────────────────
  onMount(() => {
    updateThumb();
    const ro = new ResizeObserver(updateThumb);
    ro.observe(viewportEl);

    // also observe mutations (child content changes)
    const mo = new MutationObserver(updateThumb);
    mo.observe(viewportEl, { childList: true, subtree: true });

    onCleanup(() => {
      ro.disconnect();
      mo.disconnect();
      clearTimeout(hideTimer);
    });
  });

  return (
    <div
      class={props.class}
      style={{
        position: "relative",
        overflow: "hidden",
        ...props.style,
      }}
    >
      {/* Viewport — native scroll, scrollbar hidden via CSS */}
      <div
        ref={viewportEl!}
        onScroll={onScroll}
        style={{
          width: "100%",
          height: "100%",
          overflow: "auto",
          /* Hide native scrollbar across browsers */
          "scrollbar-width": "none", // Firefox
        }}
      >
        <style>{`
          [data-scroll-viewport]::-webkit-scrollbar { display: none; }
        `}</style>
        <div data-scroll-viewport style={{ "min-height": "100%" }}>
          {props.children}
        </div>
      </div>

      {/* Custom scrollbar track */}
      <div
        ref={trackEl!}
        onClick={onTrackClick}
        class="w-1 overflow-hidden rounded-full"
        style={{
          position: "absolute",
          top: "2px",
          right: "4px",
          bottom: "2px",
          opacity: visible() ? 1 : 0,
          transition: "opacity 200ms ease",
          "pointer-events": visible() ? "auto" : "none",
          "z-index": 1,
        }}
      >
        {/* Thumb */}
        <div
          ref={thumbEl!}
          onPointerDown={onThumbPointerDown}
          onPointerMove={onThumbPointerMove}
          onPointerUp={onThumbPointerUp}
          class="bg-border rounded-full"
          style={{
            position: "absolute",
            top: `${thumbTop()}px`,
            left: 0,
            width: "100%",
            height: `${thumbHeight()}px`,
            cursor: "pointer",
            "touch-action": "none",
            transition: dragging() ? "none" : "opacity 150ms ease",
          }}
        />
      </div>
    </div>
  );
}
