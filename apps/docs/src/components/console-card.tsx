import Image from "next/image";

const LABEL =
    "Cntrl Console: manage your servers and devices from anywhere, no VPN and no open ports. Explore Console.";

/**
 * Cntrl Console's card at the foot of the docs' sidebar: the cards lab's
 * landscape sidebar card (console's brand/launch-cards/sidebar-wide-*), in the
 * docs' theme, linking to Console. It's 236 wide, the sidebar's width inside
 * its padding.
 */
export function ConsoleCard() {
    return (
        <a
            href="https://console.cntrl.pw"
            aria-label={LABEL}
            className="focus-visible:outline-fd-ring mt-3 block rounded-[14px] transition-opacity duration-150 hover:opacity-90 focus-visible:outline-2 focus-visible:outline-offset-2"
        >
            <Image
                src="/console-card-light.png"
                alt=""
                width={236}
                height={140}
                className="block h-auto w-full dark:hidden"
            />
            <Image
                src="/console-card-dark.png"
                alt=""
                width={236}
                height={140}
                className="hidden h-auto w-full dark:block"
            />
        </a>
    );
}
