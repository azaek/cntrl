import { JSX } from "solid-js/h/jsx-runtime";
import { Tooltip, TooltipContent, TooltipTrigger } from "./tooltip";

const ContentTip = ({
  children,
  content,
  side,
}: {
  children: JSX.Element;
  content: JSX.Element;
  side?: "top" | "right" | "bottom" | "left";
}) => {
  return (
    <Tooltip placement={side}>
      <TooltipTrigger class="cursor-pointer">
        <>{children}</>
      </TooltipTrigger>
      <TooltipContent class="max-w-3xs">
        <>{content}</>
      </TooltipContent>
    </Tooltip>
  );
};

export default ContentTip;
