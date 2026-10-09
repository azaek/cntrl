"use client";

import { usePathname } from "next/navigation";

import { ConsoleCard } from "./console-card";

/** The sidebar's Console card, left out of Console's own docs, where its readers know Console already. */
export function ConsoleSidebarCard() {
    const pathname = usePathname();
    return pathname.startsWith("/docs/console") ? null : <ConsoleCard />;
}
