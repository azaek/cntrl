import { cn } from "./utils";

const Switch = (props: {
  value: boolean;
  onValueChange: (val: boolean) => void;
  class?: string;
}) => {
  return (
    <div class="flex items-center justify-end gap-2">
      <p class="text-[10px] font-semibold text-neutral-600 uppercase">
        {props.value ? "On" : "Off"}
      </p>
      <button
        type="button"
        role="switch"
        aria-checked={props.value}
        onClick={() => props.onValueChange(!props.value)}
        class={cn(
          "peer inline-flex h-5.5 w-10 shrink-0 cursor-pointer items-center rounded-md border-2 border-transparent bg-neutral-950 ring-2 transition-colors focus-visible:outline-none disabled:cursor-not-allowed disabled:opacity-50",
          props.value ? "ring-border" : "ring-white/5",
          props.class,
        )}
      >
        <span
          class={cn(
            "pointer-events-none block h-4.5 w-4 rounded-sm shadow-lg ring-0 transition-all",
            props.value
              ? "translate-x-5 bg-neutral-300"
              : "translate-x-[calc(0%)] bg-neutral-800",
          )}
        />
      </button>
    </div>
  );
};

export { Switch };
