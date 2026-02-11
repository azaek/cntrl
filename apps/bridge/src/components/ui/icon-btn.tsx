import type { JSX } from "solid-js/jsx-runtime";
import { cn } from "../utils";

export const IconButton = ({
  children,
  onClick,
  className,
  disabled = false,
}: {
  children: JSX.Element | JSX.Element[];
  onClick: () => void;
  className?: string;
  disabled?: boolean;
}) => {
  return (
    <button
      onClick={onClick}
      disabled={disabled}
      class={cn(
        "size-6 rounded border bg-neutral-950/5 text-xs font-medium text-white shadow-xs ring-0 transition-colors outline-none",
        "flex cursor-pointer items-center justify-center backdrop-blur-xs enabled:hover:bg-neutral-800/50",
        "disabled:cursor-not-allowed disabled:opacity-40 [&_svg:not([class*='size-'])]:size-4",
        className,
      )}
    >
      {children}
    </button>
  );
};
