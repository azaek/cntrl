"use client";

import { cn } from "@/lib/cn";
import { SidebarItem } from "./layout/docs/sidebar";
import type { SidebarPageTreeComponents } from "./layout/sidebar/page-tree";

// 'New', 'Updated', 'Deprecated', 'Beta', 'Experimental'

export const SidebarBadge: SidebarPageTreeComponents["Item"] = ({ item, ...props }) => {
  // @ts-expect-error -- badge is injected dynamically
  const badge = item.badge as string | undefined;

  return (
    <div className="group/badge relative">
      <SidebarItem href={item.url} icon={item.icon} external={item.external} {...props}>
        {item.name}
      </SidebarItem>
      {badge === "New" && (
        <span
          className={cn(
            "bg-fd-primary text-fd-primary-foreground absolute end-2 top-1/2 min-w-5 -translate-y-1/2 rounded-sm px-1 py-1 text-[10px] leading-none font-medium",
          )}
        >
          New
        </span>
      )}
      {badge === "Updated" && (
        <span
          className={cn(
            "bg-fd-accent text-fd-accent-foreground absolute end-2 top-1/2 min-w-5 -translate-y-1/2 rounded-sm px-1 py-1 text-[10px] leading-none font-medium",
          )}
        >
          Updated
        </span>
      )}
      {badge === "Deprecated" && (
        <span
          className={cn(
            "bg-fd-primary/20 text-fd-primary-foreground absolute end-2 top-1/2 min-w-5 -translate-y-1/2 rounded-sm px-1 py-1 text-[10px] leading-none font-medium",
          )}
        >
          Deprecated
        </span>
      )}
      {badge === "Beta" && (
        <span
          className={cn(
            "bg-fd-secondary text-fd-secondary-foreground absolute end-2 top-1/2 min-w-5 -translate-y-1/2 rounded-sm px-1 py-1 text-[10px] leading-none font-medium",
          )}
        >
          Beta
        </span>
      )}
      {badge === "Experimental" && (
        <span
          className={cn(
            "bg-fd-diff-remove text-fd-foreground absolute end-2 top-1/2 min-w-5 -translate-y-1/2 rounded-sm px-1 py-1 text-[10px] leading-none font-medium",
          )}
        >
          Experimental
        </span>
      )}
    </div>
  );
};
