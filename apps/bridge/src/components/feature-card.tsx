import type { JSX } from "solid-js/jsx-runtime";
import { Switch } from "./switch";
import { cn } from "./utils";

const FeatureCard = (props: {
  id?: string;
  className?: string;
  icon: JSX.Element;
  title: string;
  description: string;
  value: boolean;
  onValueChange: (value: boolean) => void;
}) => {
  return (
    <div
      class={cn(
        "flex w-full items-center gap-2 rounded px-2 py-1.5 transition-all [&_svg:not([class*='size-'])]:size-4.5",
        props.value ? "text-white" : "text-fg-muted",
      )}
    >
      {props.icon}
      <div class="flex flex-1 flex-col items-start">
        <p class="text-sm font-medium">{props.title}</p>
        <p class="text-xs opacity-40">{props.description}</p>
      </div>
      <Switch value={props.value} onValueChange={props.onValueChange} />
    </div>
  );
};

export default FeatureCard;
