"use client";
import { cva } from "class-variance-authority";
import { type ComponentProps, use, useRef } from "react";
import { cn } from "../../../lib/cn";
import { mergeRefs } from "../../../lib/merge-refs";
import * as Base from "../sidebar/base";
import { createLinkItemRenderer } from "../sidebar/link-item";
import { createPageTreeRenderer } from "../sidebar/page-tree";
import { LayoutContext } from "./client";

const itemVariants = cva(
  "relative flex flex-row items-center gap-2 rounded-lg p-2 text-start text-fd-muted-foreground wrap-anywhere [&_svg]:size-4 [&_svg]:shrink-0",
  {
    variants: {
      variant: {
        link: "transition-colors hover:bg-fd-accent/50 hover:text-fd-accent-foreground/80 hover:transition-none data-[active=true]:bg-fd-primary/10 data-[active=true]:text-fd-primary data-[active=true]:hover:transition-colors",
        button:
          "transition-colors hover:bg-fd-accent/50 hover:text-fd-accent-foreground/80 hover:transition-none",
      },
      highlight: {
        true: "data-[active=true]:before:content-[''] data-[active=true]:before:bg-fd-primary data-[active=true]:before:absolute data-[active=true]:before:w-px data-[active=true]:before:inset-y-2.5 data-[active=true]:before:start-2.5",
      },
    },
  },
);

function getItemOffset(depth: number) {
  return `calc(${2 + 3 * depth} * var(--spacing))`;
}

export function Sidebar(props: ComponentProps<typeof Base.SidebarProvider>) {
  return <Base.SidebarProvider {...props} />;
}

export function SidebarFolder(props: ComponentProps<typeof Base.SidebarFolder>) {
  return <Base.SidebarFolder {...props} />;
}

export function SidebarCollapseTrigger(
  props: ComponentProps<typeof Base.SidebarCollapseTrigger>,
) {
  return <Base.SidebarCollapseTrigger {...props} />;
}

export function SidebarViewport(props: ComponentProps<typeof Base.SidebarViewport>) {
  return <Base.SidebarViewport {...props} />;
}

export function SidebarTrigger(props: ComponentProps<typeof Base.SidebarTrigger>) {
  return <Base.SidebarTrigger {...props} />;
}

export function SidebarContent({
  ref: refProp,
  className,
  children,
  ...props
}: ComponentProps<"aside">) {
  const { navMode } = use(LayoutContext)!;
  const ref = useRef<HTMLElement>(null);

  return (
    <Base.SidebarContent>
      {({ collapsed, hovered, ref: asideRef, ...rest }) => (
        <div
          data-sidebar-placeholder=""
          className={cn(
            "md:layout:[--fd-sidebar-width:268px] pointer-events-none sticky z-20 [grid-area:sidebar] *:pointer-events-auto max-md:hidden",
            navMode === "auto"
              ? "top-(--fd-docs-row-1) h-[calc(var(--fd-docs-height)-var(--fd-docs-row-1))]"
              : "top-(--fd-docs-row-2) h-[calc(var(--fd-docs-height)-var(--fd-docs-row-2))]",
          )}
        >
          {collapsed && <div className="absolute inset-y-0 start-0 w-4" {...rest} />}
          <aside
            id="nd-sidebar"
            ref={mergeRefs(ref, refProp, asideRef)}
            data-collapsed={collapsed}
            data-hovered={collapsed && hovered}
            className={cn(
              "absolute inset-y-0 start-0 flex w-full flex-col items-end text-sm duration-250 *:w-(--fd-sidebar-width)",
              navMode === "auto" && "bg-fd-card border-e",
              collapsed && [
                "bg-fd-card inset-y-2 w-(--fd-sidebar-width) rounded-xl border transition-transform",
                hovered
                  ? "translate-x-2 shadow-lg rtl:-translate-x-2"
                  : "-translate-x-(--fd-sidebar-width) rtl:translate-x-full",
              ],
              ref.current &&
                (ref.current.getAttribute("data-collapsed") === "true") !== collapsed &&
                "transition-[width,inset-block,translate,background-color]",
              className,
            )}
            {...props}
            {...rest}
          >
            {children}
          </aside>
        </div>
      )}
    </Base.SidebarContent>
  );
}

export function SidebarDrawer({
  children,
  className,
  ...props
}: ComponentProps<typeof Base.SidebarDrawerContent>) {
  return (
    <>
      <Base.SidebarDrawerOverlay className="data-[state=open]:animate-fd-fade-in data-[state=closed]:animate-fd-fade-out fixed inset-0 z-40 backdrop-blur-xs" />
      <Base.SidebarDrawerContent
        className={cn(
          "bg-fd-background data-[state=open]:animate-fd-sidebar-in data-[state=closed]:animate-fd-sidebar-out fixed inset-y-0 end-0 z-40 flex w-[85%] max-w-[380px] flex-col border-s text-[0.9375rem] shadow-lg",
          className,
        )}
        {...props}
      >
        {children}
      </Base.SidebarDrawerContent>
    </>
  );
}

export function SidebarSeparator({
  className,
  style,
  children,
  ...props
}: ComponentProps<"p">) {
  const depth = Base.useFolderDepth();

  return (
    <Base.SidebarSeparator
      className={cn("[&_svg]:size-4 [&_svg]:shrink-0", className)}
      style={{
        paddingInlineStart: getItemOffset(depth),
        ...style,
      }}
      {...props}
    >
      {children}
    </Base.SidebarSeparator>
  );
}

export function SidebarItem({
  className,
  style,
  children,
  ...props
}: ComponentProps<typeof Base.SidebarItem>) {
  const depth = Base.useFolderDepth();

  return (
    <Base.SidebarItem
      className={cn(itemVariants({ variant: "link", highlight: depth >= 1 }), className)}
      style={{
        paddingInlineStart: getItemOffset(depth),
        ...style,
      }}
      {...props}
    >
      {children}
    </Base.SidebarItem>
  );
}

export function SidebarFolderTrigger({
  className,
  style,
  ...props
}: ComponentProps<typeof Base.SidebarFolderTrigger>) {
  const { depth, collapsible } = Base.useFolder()!;

  return (
    <Base.SidebarFolderTrigger
      className={cn(
        itemVariants({ variant: collapsible ? "button" : null }),
        "w-full",
        className,
      )}
      style={{
        paddingInlineStart: getItemOffset(depth - 1),
        ...style,
      }}
      {...props}
    >
      {props.children}
    </Base.SidebarFolderTrigger>
  );
}

export function SidebarFolderLink({
  className,
  style,
  ...props
}: ComponentProps<typeof Base.SidebarFolderLink>) {
  const depth = Base.useFolderDepth();

  return (
    <Base.SidebarFolderLink
      className={cn(
        itemVariants({ variant: "link", highlight: depth > 1 }),
        "w-full",
        className,
      )}
      style={{
        paddingInlineStart: getItemOffset(depth - 1),
        ...style,
      }}
      {...props}
    >
      {props.children}
    </Base.SidebarFolderLink>
  );
}

export function SidebarFolderContent({
  className,
  children,
  ...props
}: ComponentProps<typeof Base.SidebarFolderContent>) {
  const depth = Base.useFolderDepth();

  return (
    <Base.SidebarFolderContent
      className={cn(
        "relative",
        depth === 1 &&
          "before:bg-fd-border before:absolute before:inset-y-1 before:start-2.5 before:w-px before:content-['']",
        className,
      )}
      {...props}
    >
      {children}
    </Base.SidebarFolderContent>
  );
}
export const SidebarPageTree = createPageTreeRenderer({
  SidebarFolder,
  SidebarFolderContent,
  SidebarFolderLink,
  SidebarFolderTrigger,
  SidebarItem,
  SidebarSeparator,
});

export const SidebarLinkItem = createLinkItemRenderer({
  SidebarFolder,
  SidebarFolderContent,
  SidebarFolderLink,
  SidebarFolderTrigger,
  SidebarItem,
});
