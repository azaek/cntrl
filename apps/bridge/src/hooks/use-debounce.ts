import { createEffect, onCleanup, type Accessor } from "solid-js";

export function useDebounce<T>(
  value: Accessor<T>,
  callback: (value: T) => void,
  delay: number,
) {
  createEffect(() => {
    const v = value();
    const timer = setTimeout(() => {
      callback(v);
    }, delay);

    onCleanup(() => clearTimeout(timer));
  });
}
