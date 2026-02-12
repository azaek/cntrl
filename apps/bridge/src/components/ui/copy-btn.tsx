import { Check, Copy } from "lucide-solid";
import { createSignal, Show } from "solid-js";
import { cn } from "../utils";
import TextTip from "./text-tip";

const CopyBtn = ({
  content,
  className,
  disabled,
  tip,
}: {
  content: string;
  className?: string;
  disabled?: boolean;
  tip?: string;
}) => {
  const [copied, setCopied] = createSignal(false);

  const copy = () => {
    navigator.clipboard.writeText(content);
    setCopied(true);
    setTimeout(() => {
      setCopied(false);
    }, 2000);
  };

  return (
    <TextTip content={tip || "Copy"} side="top">
      <button
        onClick={copy}
        disabled={disabled}
        class={cn(
          "flex cursor-pointer items-center justify-center disabled:cursor-not-allowed disabled:opacity-50",
          "initial:size-6 border-none outline-none [&_svg:not([class*='size-'])]:size-4",
          className,
        )}
      >
        <Show when={copied()} fallback={<Copy />}>
          <Check />
        </Show>
      </button>
    </TextTip>
  );
};

export default CopyBtn;
