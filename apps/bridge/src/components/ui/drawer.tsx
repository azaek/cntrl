import type { Component, ComponentProps, JSX, ValidComponent } from "solid-js";
import { splitProps } from "solid-js";

import type {
  ContentProps,
  DescriptionProps,
  DynamicProps,
  LabelProps,
  OverlayProps,
} from "@corvu/drawer";
import DrawerPrimitive from "@corvu/drawer";

import { cn } from "../utils";

const Drawer = DrawerPrimitive;

const DrawerTrigger = DrawerPrimitive.Trigger;

const DrawerPortal = DrawerPrimitive.Portal;

const DrawerClose = DrawerPrimitive.Close;

type DrawerOverlayProps<T extends ValidComponent = "div"> = OverlayProps<T> & {
  class?: string;
};

const DrawerOverlay = <T extends ValidComponent = "div">(
  props: DynamicProps<T, DrawerOverlayProps<T>>,
) => {
  const [, rest] = splitProps(props as DrawerOverlayProps, ["class"]);
  const drawerContext = DrawerPrimitive.useContext();
  return (
    <DrawerPrimitive.Overlay
      class={cn(
        "fixed inset-0 z-50 transition-all data-transitioning:transition-colors data-transitioning:duration-300",
        props.class,
      )}
      style={{
        "background-color": `rgb(23 23 23 / ${0.4 * drawerContext.openPercentage()})`,
        "backdrop-filter": `blur(${drawerContext.openPercentage() * 4}px)`,
      }}
      {...rest}
    />
  );
};

type DrawerContentProps<T extends ValidComponent = "div"> = ContentProps<T> & {
  class?: string;
  children?: JSX.Element;
};

const DrawerContent = <T extends ValidComponent = "div">(
  props: DynamicProps<T, DrawerContentProps<T>>,
) => {
  const [, rest] = splitProps(props as DrawerContentProps, ["class", "children"]);
  return (
    <DrawerPortal>
      <DrawerOverlay />
      <DrawerPrimitive.Content
        class={cn(
          "fixed inset-x-2 bottom-2 z-50 mt-24 flex h-auto flex-col overflow-hidden rounded-xl border bg-neutral-900 data-transitioning:transition-transform data-transitioning:duration-300 md:select-none",
          props.class,
        )}
        {...rest}
      >
        <div class="mx-auto mt-2 h-1 w-10 rounded-full bg-neutral-800" />
        {props.children}
      </DrawerPrimitive.Content>
    </DrawerPortal>
  );
};

const DrawerHeader: Component<ComponentProps<"div">> = (props) => {
  const [, rest] = splitProps(props, ["class"]);
  return (
    <div class={cn("grid gap-1.5 p-4 text-center sm:text-left", props.class)} {...rest} />
  );
};

const DrawerFooter: Component<ComponentProps<"div">> = (props) => {
  const [, rest] = splitProps(props, ["class"]);
  return <div class={cn("t-auto flex flex-col gap-2 p-4", props.class)} {...rest} />;
};

type DrawerTitleProps<T extends ValidComponent = "div"> = LabelProps<T> & {
  class?: string;
};

const DrawerTitle = <T extends ValidComponent = "div">(
  props: DynamicProps<T, DrawerTitleProps<T>>,
) => {
  const [, rest] = splitProps(props as DrawerTitleProps, ["class"]);
  return (
    <DrawerPrimitive.Label
      class={cn("text-lg leading-none font-semibold tracking-tight", props.class)}
      {...rest}
    />
  );
};

type DrawerDescriptionProps<T extends ValidComponent = "div"> = DescriptionProps<T> & {
  class?: string;
};

const DrawerDescription = <T extends ValidComponent = "div">(
  props: DynamicProps<T, DrawerDescriptionProps<T>>,
) => {
  const [, rest] = splitProps(props as DrawerDescriptionProps, ["class"]);
  return (
    <DrawerPrimitive.Description
      class={cn("text-muted-foreground text-sm", props.class)}
      {...rest}
    />
  );
};

export {
  Drawer,
  DrawerClose,
  DrawerContent,
  DrawerDescription,
  DrawerFooter,
  DrawerHeader,
  DrawerOverlay,
  DrawerPortal,
  DrawerTitle,
  DrawerTrigger,
};
