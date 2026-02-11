import type { JSX } from "solid-js/jsx-runtime";
import ScrollArea from "./scroll-area";
const Container = (props: {
  children: JSX.Element | JSX.Element[];
  className?: string;
}) => {
  return (
    <ScrollArea class="min-h-0 w-full flex-1">
      <div
        class={`flex w-full flex-1 flex-col items-center gap-0.5 ${props.className || ""}`}
      >
        {props.children}
      </div>
    </ScrollArea>
  );

  return (
    <div
      class={`flex w-full flex-1 flex-col items-center gap-0.5 overflow-y-auto ${props.className || ""}`}
    >
      {props.children}
    </div>
  );
};
export default Container;
