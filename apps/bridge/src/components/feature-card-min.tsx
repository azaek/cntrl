import type { JSX } from "solid-js/jsx-runtime";
import { Switch } from "./switch";
import TextTip from "./ui/text-tip";
import { cn } from "./utils";

const FeatureCardMin = (props: {
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
        "flex w-full items-center gap-2 rounded px-2 py-1.5 transition-all [&_svg:not([class*='size-'])]:size-3.5",
        props.value ? "text-neutral-300" : "text-neutral-500",
      )}
    >
      {props.icon}
      <div class="flex flex-1 flex-col items-start">
        <TextTip content={props.description} side="top">
          <p class="text-sm font-medium underline decoration-dotted underline-offset-2">
            {props.title}
          </p>
        </TextTip>
      </div>
      <Switch value={props.value} onValueChange={props.onValueChange} />
    </div>
  );
};

export default FeatureCardMin;
