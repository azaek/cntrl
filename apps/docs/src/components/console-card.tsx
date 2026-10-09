import Image from "next/image";

const LABEL =
    "Cntrl Console: manage your servers and devices from anywhere, no VPN and no open ports. Explore Console.";

/**
 * The cards lab's cards (console's brand/launch-cards), each 236 wide: the
 * width inside the sidebar's padding, and near the "On this page" column's.
 * Corners are in percent, so they stay the cards' 14 px at any width.
 */
const CARDS = {
    /**
     * The sidebar's foot: the landscape sidebar card (sidebar-wide-*). Where
     * the "On this page" card shows, this one steps aside: one card a page.
     */
    sidebar: {
        src: "console-card",
        height: 140,
        className:
            "mt-3 rounded-[5.93%/10%] xl:[@media(min-height:801px)]:[body:has(#nd-toc_.console-toc)_&]:hidden",
    },
    /**
     * Under "On this page" (toc-*). Hidden on short screens, where the
     * headings need the room.
     */
    toc: {
        src: "console-card-toc",
        height: 300,
        className:
            "console-toc mt-6 rounded-[5.93%/4.67%] [@media(max-height:800px)]:hidden",
    },
} as const;

/** Cntrl Console's card, in the docs' theme, linking to Console. */
export function ConsoleCard({ size = "sidebar" }: { size?: keyof typeof CARDS }) {
    const card = CARDS[size];
    return (
        <a
            href="https://console.cntrl.pw"
            aria-label={LABEL}
            className={`focus-visible:outline-fd-ring block shrink-0 transition-opacity duration-150 hover:opacity-90 focus-visible:outline-2 focus-visible:outline-offset-2 ${card.className}`}
        >
            <Image
                src={`/${card.src}-light.png`}
                alt=""
                width={236}
                height={card.height}
                className="block h-auto w-full dark:hidden"
            />
            <Image
                src={`/${card.src}-dark.png`}
                alt=""
                width={236}
                height={card.height}
                className="hidden h-auto w-full dark:block"
            />
        </a>
    );
}
