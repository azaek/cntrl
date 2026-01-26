import type { JSX } from "solid-js/jsx-runtime";
import { cn } from "./utils";

export const Button = ({
  children,
  onClick,
  className,
}: {
  children: JSX.Element | JSX.Element[];
  onClick: () => void;
  className?: string;
}) => {
  return (
    <button
      onClick={onClick}
      class={cn(
        "active:bg-toggle-bg min-h-9 flex-1 rounded border-none px-3 py-2 text-xs font-medium text-white shadow-xs ring-0 transition-colors outline-none",
        "hover:bg-card/80 flex items-center gap-2",
        "[&_svg:not([class*='size-'])]:size-4",
        className,
      )}
    >
      {children}
    </button>
  );
};
