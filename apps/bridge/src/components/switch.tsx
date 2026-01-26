import { cn } from "./utils";

const Switch = (props: {
  value: boolean;
  onValueChange: (val: boolean) => void;
  class?: string;
}) => {
  return (
    <div class="flex items-center justify-end gap-2">
      <p class="text-fg-muted/50 text-xs">{props.value ? "On" : "Off"}</p>
      <button
        type="button"
        role="switch"
        aria-checked={props.value}
        onClick={() => props.onValueChange(!props.value)}
        class={cn(
          "peer inline-flex h-5 w-10 shrink-0 cursor-pointer items-center rounded border-2 border-transparent bg-[#1A1A1A] ring-2 transition-colors focus-visible:outline-none disabled:cursor-not-allowed disabled:opacity-50",
          props.value ? "ring-white/10" : "ring-white/5",
          props.class,
        )}
      >
        <span
          class={cn(
            "pointer-events-none block h-4 w-4 rounded-xs shadow-lg ring-0 transition-all",
            props.value
              ? "translate-x-5 bg-white/50"
              : "translate-x-[calc(0%)] bg-white/10",
          )}
        />
      </button>
    </div>
  );
};

export { Switch };
