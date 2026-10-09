// Product analytics on cntrl.pw, as in Console (its D105): PostHog only once
// someone accepts. The choice is kept in this browser; until it's yes,
// PostHog isn't loaded at all, so nothing is sent or stored by it. What goes
// is small: pages by path and page leaves, never titles, query strings or
// anything typed. Events go through this site's /ingest, which doesn't pass
// on the visitor's IP. Browser only.

import type { PostHog } from "posthog-js";
import { useSyncExternalStore } from "react";

export type Choice = "granted" | "denied";

const CHOICE = "cntrl-analytics";

/** PostHog's project key, the same project as Console's; without it, analytics are off. */
export const ANALYTICS_KEY = process.env.NEXT_PUBLIC_POSTHOG_KEY?.trim() || null;

let choice: Choice | null | undefined;
let posthog: PostHog | null = null;
let starting: Promise<void> | null = null;
const listeners = new Set<() => void>();

function read(): Choice | null {
    try {
        const saved = window.localStorage.getItem(CHOICE);
        return saved === "granted" || saved === "denied" ? saved : null;
    } catch {
        return null;
    }
}

/** This browser's choice, or null before one; always null on the server. */
export function useAnalyticsChoice(): Choice | null {
    return useSyncExternalStore(
        (listener) => {
            listeners.add(listener);
            return () => listeners.delete(listener);
        },
        () => (choice === undefined ? (choice = read()) : choice),
        () => null,
    );
}

/** Starts analytics if this browser said yes before; the layout calls it once. */
export function startAnalytics(): void {
    if (ANALYTICS_KEY && read() === "granted") void load();
}

/** Accepts or declines. Declining after accepting stops PostHog and clears what it kept. */
export function chooseAnalytics(next: Choice): void {
    try {
        window.localStorage.setItem(CHOICE, next);
    } catch {
        // Storage can be off: the choice holds for this page only.
    }
    choice = next;
    for (const listener of listeners) listener();
    if (next === "granted") {
        if (posthog) posthog.opt_in_capturing({ captureEventName: false });
        else void load();
    } else forgetAll();
}

/** Strips what could name something from a set of properties: page titles, and query strings and fragments from URLs. */
function scrubProperties(properties: Record<string, unknown>): void {
    delete properties.$title;
    delete properties.title;
    for (const [name, value] of Object.entries(properties)) {
        if (
            typeof value === "string" &&
            /url$|referrer$/i.test(name) &&
            /^https?:\/\//.test(value)
        ) {
            const url = new URL(value);
            properties[name] = `${url.origin}${url.pathname}`;
        }
    }
}

/** Every event, before it goes: its own properties, and the ones it sets on the person. */
function scrub<
    T extends {
        properties?: Record<string, unknown>;
        $set?: Record<string, unknown>;
        $set_once?: Record<string, unknown>;
    } | null,
>(event: T): T {
    if (!event) return event;
    for (const properties of [
        event.properties,
        event.$set,
        event.$set_once,
        event.properties?.$set,
        event.properties?.$set_once,
    ]) {
        if (properties && typeof properties === "object")
            scrubProperties(properties as Record<string, unknown>);
    }
    return event;
}

async function load(): Promise<void> {
    if (!ANALYTICS_KEY || posthog) return;
    starting ??= import("posthog-js").then(({ default: instance }) => {
        instance.init(ANALYTICS_KEY!, {
            api_host: "/ingest",
            ui_host: "https://us.posthog.com",
            person_profiles: "identified_only",
            capture_pageview: "history_change",
            capture_pageleave: true,
            autocapture: false,
            capture_dead_clicks: false,
            capture_heatmaps: false,
            rageclick: false,
            capture_performance: false,
            capture_exceptions: false,
            disable_session_recording: true,
            disable_surveys: true,
            disable_product_tours: true,
            disable_web_experiments: true,
            disable_external_dependency_loading: true,
            advanced_disable_flags: true,
            mask_all_text: true,
            mask_all_element_attributes: true,
            save_campaign_params: false,
            before_send: scrub,
        });
        // The yes was given here, so PostHog hears it too.
        instance.opt_in_capturing({ captureEventName: false });
        posthog = instance;
    });
    await starting;
}

/** Stops PostHog and clears its cookie and storage. */
function forgetAll(): void {
    posthog?.opt_out_capturing();
    posthog?.reset();
    try {
        for (const name of Object.keys(window.localStorage)) {
            if (name.startsWith("ph_") || name.startsWith("__ph"))
                window.localStorage.removeItem(name);
        }
        for (const name of Object.keys(window.sessionStorage)) {
            if (name.startsWith("ph_") || name.startsWith("__ph"))
                window.sessionStorage.removeItem(name);
        }
    } catch {
        // Storage can be off, in which case PostHog kept nothing there.
    }
    for (const cookie of document.cookie.split(";")) {
        const name = cookie.split("=")[0]!.trim();
        if (name.startsWith("ph_")) document.cookie = `${name}=; path=/; max-age=0`;
    }
}
