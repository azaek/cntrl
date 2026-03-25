import gsap from "gsap";
import { Check, X } from "lucide-solid";
import { createEffect, createMemo, createSignal, onMount, type JSX } from "solid-js";
import { IconButton } from "./ui/icon-btn";
import { cn } from "./utils";

const SettingInputCard = (props: {
    icon: JSX.Element;
    title: string;
    description: string;
    value: string;
    placeholder?: string;
    inputClass?: string;
    onSave: (value: string) => Promise<void>;
    validate?: (value: string) => boolean;
}) => {
    const [draft, setDraft] = createSignal(props.value);
    let btnsRef!: HTMLDivElement;
    let inputRef!: HTMLInputElement;
    let mounted = false;

    const isDirty = createMemo(() => draft() !== props.value);

    onMount(() => {
        gsap.set(btnsRef, { autoAlpha: 0, x: 6 });
        mounted = true;
    });

    createEffect(() => {
        const dirty = isDirty();
        if (!mounted) return;
        if (dirty) {
            gsap.fromTo(
                btnsRef,
                { autoAlpha: 0, x: 6 },
                { autoAlpha: 1, x: 0, duration: 0.18, ease: "power2.out" },
            );
        } else {
            gsap.to(btnsRef, { autoAlpha: 0, x: 6, duration: 0.12, ease: "power2.in" });
        }
    });

    const handleSave = async () => {
        if (props.validate && !props.validate(draft())) {
            gsap.fromTo(
                inputRef,
                { x: -3 },
                { x: 0, duration: 0.35, ease: "elastic.out(1, 0.3)" },
            );
            return;
        }
        await props.onSave(draft());
    };

    const handleCancel = () => setDraft(props.value);

    return (
        <div class="flex w-full items-center gap-2 rounded px-2 py-1.5 text-white [&_svg:not([class*='size-'])]:size-4.5">
            {props.icon}
            <div class="flex min-w-0 flex-1 flex-col items-start">
                <p class="text-sm font-medium">{props.title}</p>
                <p class="text-xs opacity-40">{props.description}</p>
            </div>
            <div class="flex shrink-0 items-center gap-1">
                <div ref={btnsRef} class="flex items-center gap-1">
                    <IconButton
                        onClick={handleCancel}
                        class="size-6 rounded border-white/5 text-neutral-500 hover:text-white"
                    >
                        <X />
                    </IconButton>
                    <IconButton
                        onClick={handleSave}
                        class="size-6 rounded border-white/5 text-green-500 hover:text-green-400"
                    >
                        <Check />
                    </IconButton>
                </div>
                <input
                    ref={inputRef}
                    class={cn(
                        "h-8 w-20 rounded-md border border-white/10 bg-transparent px-2 text-right text-xs",
                        "placeholder:text-neutral-600 focus-visible:ring-1 focus-visible:ring-white/20 focus-visible:outline-none",
                        props.inputClass,
                    )}
                    value={draft()}
                    onInput={(e) => setDraft(e.currentTarget.value)}
                    onKeyDown={(e) => {
                        if (e.key === "Enter") handleSave();
                        if (e.key === "Escape") handleCancel();
                    }}
                    placeholder={props.placeholder}
                />
            </div>
        </div>
    );
};

export default SettingInputCard;
