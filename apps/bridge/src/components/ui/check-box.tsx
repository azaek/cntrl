import type { ValidComponent } from "solid-js";
import { Match, splitProps, Switch } from "solid-js";

import * as CheckboxPrimitive from "@kobalte/core/checkbox";
import type { PolymorphicProps } from "@kobalte/core/polymorphic";
import { cn } from "../utils";

type CheckboxRootProps<T extends ValidComponent = "div"> =
    CheckboxPrimitive.CheckboxRootProps<T> & { class?: string | undefined };

const Checkbox = <T extends ValidComponent = "div">(
    props: PolymorphicProps<T, CheckboxRootProps<T>>,
) => {
    const [local, others] = splitProps(props as CheckboxRootProps, ["class"]);
    return (
        <CheckboxPrimitive.Root
            class={cn("items-top group relative flex space-x-2", local.class)}
            {...others}
        >
            <CheckboxPrimitive.Input class="peer" />
            <CheckboxPrimitive.Control
                onClick={(e: MouseEvent) => e.stopPropagation()}
                class="peer-focus-visible:ring-border size-4 shrink-0 rounded-sm border border-neutral-500 peer-focus-visible:ring-1 peer-focus-visible:outline-none disabled:cursor-not-allowed disabled:opacity-50 data-[checked]:border-none data-[checked]:bg-neutral-50 data-[checked]:text-neutral-900 data-[indeterminate]:border-none data-[indeterminate]:bg-neutral-300 data-[indeterminate]:text-neutral-800"
            >
                <CheckboxPrimitive.Indicator>
                    <Switch>
                        <Match when={!others.indeterminate}>
                            <svg
                                xmlns="http://www.w3.org/2000/svg"
                                viewBox="0 0 24 24"
                                fill="none"
                                stroke="currentColor"
                                stroke-width="3"
                                stroke-linecap="round"
                                stroke-linejoin="round"
                                class="size-4"
                            >
                                <path d="M5 12l5 5l10 -10" />
                            </svg>
                        </Match>
                        <Match when={others.indeterminate}>
                            <svg
                                xmlns="http://www.w3.org/2000/svg"
                                viewBox="0 0 24 24"
                                fill="none"
                                stroke="currentColor"
                                stroke-width="3"
                                stroke-linecap="round"
                                stroke-linejoin="round"
                                class="size-4"
                            >
                                <path d="M5 12l14 0" />
                            </svg>
                        </Match>
                    </Switch>
                </CheckboxPrimitive.Indicator>
            </CheckboxPrimitive.Control>
        </CheckboxPrimitive.Root>
    );
};

export { Checkbox };
