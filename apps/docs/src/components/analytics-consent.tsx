"use client";

// The analytics choice on cntrl.pw: a small banner until someone chooses,
// Accept and Decline alike, and a switch on the privacy policy to change it.

import { useEffect, useId, useState } from "react";

import {
    ANALYTICS_KEY,
    chooseAnalytics,
    startAnalytics,
    useAnalyticsChoice,
} from "@/lib/analytics";

const WHAT =
    "cntrl.pw tells PostHog, in the US, which pages you read. Nothing you type, and no titles or search terms.";

const BUTTON =
    "flex-1 cursor-pointer rounded-md border px-3 py-1.5 text-sm font-medium transition-colors duration-150 hover:bg-fd-accent focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-fd-ring";

/** Whether the page has hydrated, so a choice kept in this browser can show. */
function useHydrated(): boolean {
    const [hydrated, setHydrated] = useState(false);
    useEffect(() => setHydrated(true), []);
    return hydrated;
}

export function AnalyticsConsent() {
    const hydrated = useHydrated();
    const choice = useAnalyticsChoice();
    useEffect(() => startAnalytics(), []);
    if (!ANALYTICS_KEY || !hydrated || choice !== null) return null;
    return (
        <section
            aria-label="Analytics"
            className="fixed inset-x-4 bottom-4 z-40 pb-[env(safe-area-inset-bottom)] sm:right-auto sm:w-96"
        >
            <div className="bg-fd-popover text-fd-popover-foreground flex flex-col gap-3 rounded-xl border p-4 shadow-lg">
                <div className="flex flex-col gap-1">
                    <h2 className="text-sm font-medium">Help improve cntrl</h2>
                    <p className="text-fd-muted-foreground text-xs">
                        With your yes, {WHAT} You can change this later.{" "}
                        <a
                            href="/docs/legal/privacy#product-analytics-only-if-you-say-yes"
                            className="hover:text-fd-foreground underline underline-offset-2"
                        >
                            Privacy policy
                        </a>
                    </p>
                </div>
                <div className="flex gap-2">
                    <button
                        type="button"
                        className={BUTTON}
                        onClick={() => chooseAnalytics("denied")}
                    >
                        Decline
                    </button>
                    <button
                        type="button"
                        className={BUTTON}
                        onClick={() => chooseAnalytics("granted")}
                    >
                        Accept
                    </button>
                </div>
            </div>
        </section>
    );
}

/** The same choice on the privacy policy, for this browser. */
export function AnalyticsSetting() {
    const hydrated = useHydrated();
    const choice = useAnalyticsChoice();
    const id = useId();
    if (!ANALYTICS_KEY) return null;
    const on = hydrated && choice === "granted";
    return (
        <div className="not-prose flex items-start justify-between gap-3 rounded-lg border p-4 text-sm">
            <label htmlFor={id} className="flex min-w-0 flex-col gap-0.5">
                <span className="font-medium">Analytics on cntrl.pw</span>
                <span className="text-fd-muted-foreground text-xs">
                    {WHAT} From this browser only.
                </span>
            </label>
            <button
                id={id}
                type="button"
                role="switch"
                aria-checked={on}
                onClick={() => chooseAnalytics(on ? "denied" : "granted")}
                className={`focus-visible:outline-fd-ring relative h-5 w-9 shrink-0 cursor-pointer rounded-full transition-colors duration-150 focus-visible:outline-2 focus-visible:outline-offset-2 ${on ? "bg-fd-primary" : "bg-fd-muted-foreground/30"}`}
            >
                <span
                    className={`bg-fd-background absolute top-0.5 left-0.5 size-4 rounded-full shadow transition-transform duration-150 ${on ? "translate-x-4" : ""}`}
                />
            </button>
        </div>
    );
}
