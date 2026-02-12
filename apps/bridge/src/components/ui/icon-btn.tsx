import { splitProps, type ComponentProps } from "solid-js";
import { cn } from "../utils";

type IconButtonProps = ComponentProps<"button"> & {
  className?: string;
};

export const IconButton = (props: IconButtonProps) => {
  const [local, others] = splitProps(props, ["children", "className", "class"]);
  return (
    <button
      {...others}
      class={cn(
        "pointer-events-auto size-8 rounded-lg border bg-neutral-950/25 text-xs font-medium text-white shadow-xs ring-0 transition-colors outline-none",
        "flex cursor-pointer items-center justify-center backdrop-blur-xs enabled:hover:bg-neutral-800/50",
        "disabled:cursor-not-allowed disabled:opacity-40 [&_svg:not([class*='size-'])]:size-4",
        local.className,
        local.class,
      )}
    >
      {local.children}
    </button>
  );
};

export const IconButtonGhost = (props: IconButtonProps) => {
  const [local, others] = splitProps(props, ["children", "className", "class"]);
  return (
    <button
      {...others}
      class={cn(
        "pointer-events-auto size-8 rounded-md border-none text-xs font-medium text-white shadow-xs ring-0 transition-colors outline-none",
        "flex cursor-pointer items-center justify-center enabled:hover:bg-neutral-800/50",
        "disabled:cursor-not-allowed disabled:opacity-40 data-expanded:bg-neutral-700/20 [&_svg:not([class*='size-'])]:size-4",
        local.className,
        local.class,
      )}
    >
      {local.children}
    </button>
  );
};
