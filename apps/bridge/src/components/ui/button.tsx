import type { JSX } from "solid-js/jsx-runtime";
import { cn } from "../utils";

export const Button = ({
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
        "enabled:active:bg-toggle-bg min-h-9 flex-1 rounded border-none px-3 py-2 text-xs font-medium text-white shadow-xs ring-0 transition-colors outline-none",
        "enabled:hover:bg-card/80 flex cursor-pointer items-center gap-2",
        "disabled:cursor-not-allowed disabled:opacity-40 [&_svg:not([class*='size-'])]:size-4",
        className,
      )}
    >
      {children}
    </button>
  );
};
