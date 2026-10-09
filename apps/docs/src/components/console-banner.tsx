import Image from "next/image";

const LABEL =
    "Cntrl Console: manage your servers and devices from anywhere, no VPN and no open ports. Explore Console.";

/**
 * Cntrl Console at the top of the home page, above Bridge: the cards lab's
 * banner (console's brand/launch-cards/banner-*), and on phones its landscape
 * sidebar card (sidebar-wide-*), whose words stay readable at that width.
 * SVG, so both stay sharp at any size; the words are drawn as paths. The
 * corners match the cards' 14 px at whatever size they're drawn, and lazy
 * loading means only the card on screen is fetched.
 */
export function ConsoleBanner() {
    return (
        <section
            aria-label="Cntrl Console"
            className="w-full max-w-350 px-6 pt-6 min-[1400px]:border-x sm:pt-10"
        >
            <a
                href="https://console.cntrl.pw"
                aria-label={LABEL}
                className="focus-visible:outline-fd-ring mx-auto block max-w-90 rounded-[5.93%/10%] transition-opacity duration-150 hover:opacity-90 focus-visible:outline-2 focus-visible:outline-offset-2 sm:max-w-180 sm:rounded-[1.94%/7%]"
            >
                <Card
                    name="console-card-wide"
                    width={236}
                    height={140}
                    className="sm:hidden"
                />
                <Card
                    name="console-banner"
                    width={720}
                    height={200}
                    className="max-sm:hidden"
                />
            </a>
        </section>
    );
}

function Card({
    name,
    width,
    height,
    className,
}: {
    name: string;
    width: number;
    height: number;
    className: string;
}) {
    return (
        <span className={`block ${className}`}>
            <Image
                src={`/${name}-light.svg`}
                alt=""
                width={width}
                height={height}
                unoptimized
                className="block h-auto w-full dark:hidden"
            />
            <Image
                src={`/${name}-dark.svg`}
                alt=""
                width={width}
                height={height}
                unoptimized
                className="hidden h-auto w-full dark:block"
            />
        </span>
    );
}
