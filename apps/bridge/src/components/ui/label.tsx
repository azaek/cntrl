import type { Component, ComponentProps } from "solid-js";
import { splitProps } from "solid-js";
import { cn } from "../utils";

const Label: Component<ComponentProps<"label">> = (props) => {
  const [local, others] = splitProps(props, ["class"]);
  return (
    <label
      class={cn(
        "text-sm leading-none font-medium text-neutral-300 peer-disabled:cursor-not-allowed peer-disabled:opacity-70",
        "flex items-center gap-2",
        local.class,
      )}
      {...others}
    />
  );
};
const LabelSm: Component<ComponentProps<"label">> = (props) => {
  const [local, others] = splitProps(props, ["class"]);
  return (
    <label
      class={cn(
        "text-xs leading-none font-medium text-neutral-500 peer-disabled:cursor-not-allowed peer-disabled:opacity-70",
        "flex items-center gap-2",
        local.class,
      )}
      {...others}
    />
  );
};

export { Label, LabelSm };
