import { SquareDashedMousePointer } from "lucide-solid";
import { Button } from "../ui/button";

const PostContentBlock = () => {
    return (
        <div class="relative flex min-h-50 w-full flex-col items-center justify-center px-2 text-neutral-500">
            <div class="z-1 flex flex-col items-center">
                <SquareDashedMousePointer class="size-5" />
                <p class="mt-2 text-xs font-medium">
                    Couldn't find what you're looking for?
                </p>
                <div class="flex items-center gap-2">
                    <Button variant={"link"} class="text-xs">
                        Docs
                    </Button>
                    <Button variant={"link"} class="text-xs">
                        Github Issues
                    </Button>
                </div>
            </div>
            <div
                class="pointer-events-none absolute inset-0 z-0"
                style={{
                    "background-image": `
                        linear-gradient(90deg, rgba(56,56,56,0.30) 1px, transparent 0),
                        linear-gradient(180deg, rgba(56,56,56,0.30) 1px, transparent 0),
                        repeating-linear-gradient(45deg, rgba(56,56,56,0.25) 0 2px, transparent 2px 6px)
                    `,
                    "background-size": "24px 24px, 24px 24px, 24px 24px",
                }}
            />
            <div class="from-background pointer-events-none absolute inset-x-0 top-0 h-1/2 w-full bg-linear-to-b to-transparent"></div>
            <div class="from-background pointer-events-none absolute inset-x-0 bottom-0 h-1/2 w-full bg-linear-to-t to-transparent"></div>
            <div class="from-background pointer-events-none absolute inset-y-0 left-0 h-full w-[20%] bg-linear-to-r to-transparent"></div>
            <div class="from-background pointer-events-none absolute inset-y-0 right-0 h-full w-[20%] bg-linear-to-l to-transparent"></div>
        </div>
    );
};

export default PostContentBlock;
