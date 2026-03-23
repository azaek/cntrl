import { createMemo } from "solid-js";
import type { JSX } from "solid-js/jsx-runtime";
import {
    Select,
    SelectContent,
    SelectItem,
    SelectTrigger,
    SelectValue,
} from "./ui/select";

export type IntervalOption = { value: number; label: string };

const IntervalCard = (props: {
    icon: JSX.Element;
    title: string;
    description: string;
    options: IntervalOption[];
    value: number;
    onChange: (value: number) => void;
}) => {
    const selected = createMemo(
        () => props.options.find((o) => o.value === props.value) ?? props.options[0],
    );

    return (
        <div class="flex w-full items-center gap-2 rounded px-2 py-1.5 text-white transition-all [&_svg:not([class*='size-'])]:size-4.5">
            {props.icon}
            <div class="flex flex-1 flex-col items-start">
                <p class="text-sm font-medium">{props.title}</p>
                <p class="text-xs opacity-40">{props.description}</p>
            </div>
            <Select<IntervalOption>
                value={selected()}
                onChange={(opt) => {
                    if (opt) props.onChange(opt.value);
                }}
                options={props.options}
                optionValue="value"
                optionTextValue="label"
                itemComponent={(itemProps) => (
                    <SelectItem item={itemProps.item}>
                        {itemProps.item.rawValue.label}
                    </SelectItem>
                )}
                placement="bottom-end"
            >
                <SelectTrigger class="h-8 w-20 border-white/10 px-2 py-1 text-xs">
                    <SelectValue<IntervalOption>>
                        {(state) => state.selectedOption()?.label}
                    </SelectValue>
                </SelectTrigger>
                <SelectContent class="bg-background" />
            </Select>
        </div>
    );
};

export default IntervalCard;
