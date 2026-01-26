import type { JSX } from "solid-js/jsx-runtime";
const Container = (props: {
  children: JSX.Element | JSX.Element[];
  className?: string;
}) => {
  return (
    <div
      class={`flex w-full flex-1 flex-col items-center gap-0.5 overflow-y-auto ${props.className || ""}`}
    >
      {props.children}
    </div>
  );
};
export default Container;
