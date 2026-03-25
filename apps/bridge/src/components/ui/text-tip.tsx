import { JSX } from "solid-js/h/jsx-runtime";
import { Tooltip, TooltipContent, TooltipTrigger } from "./tooltip";

const TextTip = ({
  children,
  content,
  side,
}: {
  children: JSX.Element;
  content: string;
  side?: "top" | "right" | "bottom" | "left";
}) => {
  return (
    <Tooltip placement={side} openDelay={1000} closeDelay={100} skipDelayDuration={500}>
      <TooltipTrigger class="cursor-pointer">
        <>{children}</>
      </TooltipTrigger>
      <TooltipContent class="max-w-3xs">{content}</TooltipContent>
    </Tooltip>
  );
};

export default TextTip;
