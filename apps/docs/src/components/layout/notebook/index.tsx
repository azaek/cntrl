import type * as PageTree from "fumadocs-core/page-tree";
import { TreeContextProvider } from "fumadocs-ui/contexts/tree";
import { Languages, Sidebar as SidebarIcon, X } from "lucide-react";
import {
  type ComponentProps,
  type FC,
  type HTMLAttributes,
  type ReactNode,
  useMemo,
} from "react";
import { cn } from "../../../lib/cn";
import { buttonVariants } from "../../ui/button";
import { LanguageToggle } from "../language-toggle";
import { LinkItem, type LinkItemType } from "../link-item";
import { LargeSearchToggle, SearchToggle } from "../search-toggle";
import { type BaseLayoutProps, renderTitleNav, resolveLinkItems } from "../shared";
import type { SidebarPageTreeComponents } from "../sidebar/page-tree";
import { getSidebarTabs, type GetSidebarTabsOptions } from "../sidebar/tabs";
import { SidebarTabsDropdown, type SidebarTabWithProps } from "../sidebar/tabs/dropdown";
import { ThemeToggle } from "../theme-toggle";
import {
  LayoutBody,
  LayoutContextProvider,
  LayoutHeader,
  LayoutHeaderTabs,
  NavbarLinkItem,
} from "./client";
import {
  Sidebar,
  SidebarCollapseTrigger,
  SidebarContent,
  SidebarDrawer,
  SidebarLinkItem,
  SidebarPageTree,
  SidebarTrigger,
  SidebarViewport,
} from "./sidebar";

export interface DocsLayoutProps extends BaseLayoutProps {
  tree: PageTree.Root;
  tabMode?: "sidebar" | "navbar";

  nav?: BaseLayoutProps["nav"] & {
    mode?: "top" | "auto";
  };

  sidebar?: SidebarOptions;

  containerProps?: HTMLAttributes<HTMLDivElement>;
}

interface SidebarOptions
  extends
    ComponentProps<"aside">,
    Pick<ComponentProps<typeof Sidebar>, "defaultOpenLevel" | "prefetch"> {
  components?: Partial<SidebarPageTreeComponents>;

  /**
   * Root Toggle options
   */
  tabs?: SidebarTabWithProps[] | GetSidebarTabsOptions | false;

  banner?: ReactNode | FC<ComponentProps<"div">>;
  footer?: ReactNode | FC<ComponentProps<"div">>;

  /**
   * Support collapsing the sidebar on desktop mode
   *
   * @defaultValue true
   */
  collapsible?: boolean;
}

export function DocsLayout(props: DocsLayoutProps) {
  const {
    tabMode = "sidebar",
    nav = {},
    sidebar: { tabs: tabOptions, defaultOpenLevel, prefetch, ...sidebarProps } = {},
    i18n = false,
    themeSwitch = {},
    tree,
  } = props;

  const navMode = nav.mode ?? "auto";
  const links = resolveLinkItems(props);
  const tabs = useMemo(() => {
    if (Array.isArray(tabOptions)) {
      return tabOptions;
    }

    if (typeof tabOptions === "object") {
      return getSidebarTabs(tree, tabOptions);
    }

    if (tabOptions !== false) {
      return getSidebarTabs(tree);
    }

    return [];
  }, [tabOptions, tree]);

  function sidebar() {
    const { banner, footer, components, collapsible = true, ...rest } = sidebarProps;

    const iconLinks = links.filter((item) => item.type === "icon");
    const Header =
      typeof banner === "function"
        ? banner
        : ({ className, ...props }: ComponentProps<"div">) => (
            <div
              className={cn("flex flex-col gap-3 p-4 pb-2 empty:hidden", className)}
              {...props}
            >
              {props.children}
              {banner}
            </div>
          );
    const Footer =
      typeof footer === "function"
        ? footer
        : ({ className, ...props }: ComponentProps<"div">) => (
            <div
              className={cn(
                "text-fd-muted-foreground hidden flex-row items-center border-t p-4 pt-2",
                iconLinks.length > 0 && "max-lg:flex",
                className,
              )}
              {...props}
            >
              {props.children}
              {footer}
            </div>
          );
    const viewport = (
      <SidebarViewport>
        {links
          .filter((item) => item.type !== "icon")
          .map((item, i, arr) => (
            <SidebarLinkItem
              key={i}
              item={item}
              className={cn("lg:hidden", i === arr.length - 1 && "mb-4")}
            />
          ))}

        <SidebarPageTree {...components} />
      </SidebarViewport>
    );

    return (
      <>
        <SidebarContent {...rest}>
          <Header>
            {navMode === "auto" && (
              <div className="flex justify-between">
                {renderTitleNav(nav, {
                  className: "inline-flex items-center gap-2.5 font-medium",
                })}
                {collapsible && (
                  <SidebarCollapseTrigger
                    className={cn(
                      buttonVariants({
                        color: "ghost",
                        size: "icon-sm",
                        className: "text-fd-muted-foreground mt-px mb-auto",
                      }),
                    )}
                  >
                    <SidebarIcon />
                  </SidebarCollapseTrigger>
                )}
              </div>
            )}
            {tabs.length > 0 && (
              <SidebarTabsDropdown
                options={tabs}
                className={cn(tabMode === "navbar" && "lg:hidden")}
              />
            )}
          </Header>
          {viewport}
          <Footer>
            {iconLinks.map((item, i) => (
              <LinkItem
                key={i}
                item={item}
                className={cn(
                  buttonVariants({
                    size: "icon-sm",
                    color: "ghost",
                    className: "lg:hidden",
                  }),
                )}
                aria-label={item.label}
              >
                {item.icon}
              </LinkItem>
            ))}
          </Footer>
        </SidebarContent>
        <SidebarDrawer {...rest}>
          <Header>
            <SidebarTrigger
              className={cn(
                buttonVariants({
                  size: "icon-sm",
                  color: "ghost",
                  className: "text-fd-muted-foreground ms-auto",
                }),
              )}
            >
              <X />
            </SidebarTrigger>
            {tabs.length > 0 && <SidebarTabsDropdown options={tabs} />}
          </Header>
          {viewport}
          <Footer
            className={cn(
              "hidden flex-row items-center justify-end",
              (i18n || themeSwitch.enabled !== false) && "flex",
              iconLinks.length > 0 && "max-lg:flex",
            )}
          >
            {iconLinks.map((item, i) => (
              <LinkItem
                key={i}
                item={item}
                className={cn(
                  buttonVariants({
                    size: "icon-sm",
                    color: "ghost",
                  }),
                  "text-fd-muted-foreground lg:hidden",
                  i === iconLinks.length - 1 && "me-auto",
                )}
                aria-label={item.label}
              >
                {item.icon}
              </LinkItem>
            ))}
            {i18n && (
              <LanguageToggle>
                <Languages className="text-fd-muted-foreground size-4.5" />
              </LanguageToggle>
            )}
            {themeSwitch.enabled !== false &&
              (themeSwitch.component ?? (
                <ThemeToggle mode={themeSwitch.mode ?? "light-dark-system"} />
              ))}
          </Footer>
        </SidebarDrawer>
      </>
    );
  }

  return (
    <TreeContextProvider tree={tree}>
      <LayoutContextProvider
        navMode={nav.mode ?? "auto"}
        tabMode={tabMode}
        navTransparentMode={nav.transparentMode}
      >
        <Sidebar defaultOpenLevel={defaultOpenLevel} prefetch={prefetch}>
          <LayoutBody {...props.containerProps}>
            {sidebar()}
            <DocsNavbar {...props} links={links} tabs={tabs} />
            {props.children}
          </LayoutBody>
        </Sidebar>
      </LayoutContextProvider>
    </TreeContextProvider>
  );
}

function DocsNavbar({
  links,
  tabs,
  tabMode = "sidebar",
  sidebar: { collapsible: sidebarCollapsible = true } = {},
  searchToggle = {},
  themeSwitch = {},
  nav = {},
  i18n,
}: DocsLayoutProps & {
  links: LinkItemType[];
  tabs: SidebarTabWithProps[];
}) {
  const navMode = nav.mode ?? "auto";
  const showLayoutTabs = tabMode === "navbar" && tabs.length > 0;

  return (
    <LayoutHeader
      id="nd-subnav"
      className={cn(
        "data-[transparent=false]:bg-fd-background/80 layout:[--fd-header-height:--spacing(14)] sticky top-(--fd-docs-row-1) z-10 flex flex-col backdrop-blur-sm transition-colors [grid-area:header]",
        showLayoutTabs && "lg:layout:[--fd-header-height:--spacing(24)]",
      )}
    >
      <div data-header-body="" className="flex h-14 gap-2 border-b px-4 md:px-6">
        <div
          className={cn(
            "items-center",
            navMode === "top" && "flex flex-1",
            navMode === "auto" && "hidden max-md:flex has-data-[collapsed=true]:md:flex",
          )}
        >
          {sidebarCollapsible && navMode === "auto" && (
            <SidebarCollapseTrigger
              className={cn(
                buttonVariants({
                  color: "ghost",
                  size: "icon-sm",
                }),
                "text-fd-muted-foreground data-[collapsed=false]:hidden max-md:hidden",
              )}
            >
              <SidebarIcon />
            </SidebarCollapseTrigger>
          )}
          {renderTitleNav(nav, {
            className: cn(
              "inline-flex items-center gap-2.5 font-semibold",
              navMode === "auto" && "md:hidden",
            ),
          })}
          {nav.children}
        </div>
        {searchToggle.enabled !== false &&
          (searchToggle.components?.lg ? (
            <div
              className={cn(
                "my-auto w-full max-md:hidden",
                navMode === "top" ? "max-w-sm rounded-xl" : "max-w-[240px]",
              )}
            >
              {searchToggle.components.lg}
            </div>
          ) : (
            <LargeSearchToggle
              hideIfDisabled
              className={cn(
                "my-auto w-full max-md:hidden",
                navMode === "top" ? "max-w-sm rounded-xl ps-2.5" : "max-w-[240px]",
              )}
            />
          ))}
        <div className="flex flex-1 items-center justify-end md:gap-2">
          <div className="flex items-center gap-6 empty:hidden max-lg:hidden">
            {links
              .filter((item) => item.type !== "icon")
              .map((item, i) => (
                <NavbarLinkItem key={i} item={item} />
              ))}
          </div>
          {links
            .filter((item) => item.type === "icon")
            .map((item, i) => (
              <LinkItem
                key={i}
                item={item}
                className={cn(
                  buttonVariants({ size: "icon-sm", color: "ghost" }),
                  "text-fd-muted-foreground max-lg:hidden",
                )}
                aria-label={item.label}
              >
                {item.icon}
              </LinkItem>
            ))}

          <div className="flex items-center md:hidden">
            {searchToggle.enabled !== false &&
              (searchToggle.components?.sm ?? (
                <SearchToggle hideIfDisabled className="p-2" />
              ))}
            <SidebarTrigger
              className={cn(
                buttonVariants({
                  color: "ghost",
                  size: "icon-sm",
                  className: "-me-1.5 p-2",
                }),
              )}
            >
              <SidebarIcon />
            </SidebarTrigger>
          </div>

          <div className="flex items-center gap-2 max-md:hidden">
            {i18n && (
              <LanguageToggle>
                <Languages className="text-fd-muted-foreground size-4.5" />
              </LanguageToggle>
            )}
            {themeSwitch.enabled !== false &&
              (themeSwitch.component ?? (
                <ThemeToggle mode={themeSwitch.mode ?? "light-dark-system"} />
              ))}
            {sidebarCollapsible && navMode === "top" && (
              <SidebarCollapseTrigger
                className={cn(
                  buttonVariants({
                    color: "secondary",
                    size: "icon-sm",
                  }),
                  "text-fd-muted-foreground -me-1.5 rounded-full",
                )}
              >
                <SidebarIcon />
              </SidebarCollapseTrigger>
            )}
          </div>
        </div>
      </div>
      {showLayoutTabs && (
        <LayoutHeaderTabs
          data-header-tabs=""
          className="h-10 overflow-x-auto border-b px-6 max-lg:hidden"
          options={tabs}
        />
      )}
    </LayoutHeader>
  );
}
