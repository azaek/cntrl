import Image from "next/image";

const LABEL =
    "Cntrl Console: manage your servers and devices from anywhere, no VPN and no open ports. Explore Console.";

/**
 * Cntrl Console on the home page. From 640 px it's above Bridge's words,
 * leaving Bridge's Mac where it was: the cards lab's banner (console's
 * brand/launch-cards/banner-*), and where the column is narrower than 512 px
 * its landscape sidebar card (sidebar-wide-*), whose words stay readable
 * there. On phones it's that card, at the top of the page (`phone`). SVG, so
 * both stay sharp at any size; the words are drawn as paths. The corners match
 * the cards' 14 px at whatever size they're drawn, and lazy loading means only
 * the card on screen is fetched.
 */
export function ConsoleBanner({ phone = false }: { phone?: boolean }) {
    if (phone) {
        return (
            <div className="w-full px-6 pt-6 sm:hidden">
                <a
                    href="https://console.cntrl.pw"
                    aria-label={LABEL}
                    className={`${LINK} mx-auto max-w-90 rounded-[5.93%/10%]`}
                >
                    <Card
                        name="console-card-wide"
                        width={236}
                        height={140}
                        className=""
                    />
                </a>
            </div>
        );
    }
    return (
        <div className="@container mb-10 w-full max-sm:hidden">
            <a
                href="https://console.cntrl.pw"
                aria-label={LABEL}
                className={`${LINK} max-w-90 rounded-[5.93%/10%] @lg:max-w-180 @lg:rounded-[1.94%/7%]`}
            >
                <Card
                    name="console-card-wide"
                    width={236}
                    height={140}
                    className="@lg:hidden"
                />
                <Card
                    name="console-banner"
                    width={720}
                    height={200}
                    className="@max-lg:hidden"
                />
            </a>
        </div>
    );
}

const LINK =
    "focus-visible:outline-fd-ring relative block transition-opacity duration-150 hover:opacity-90 focus-visible:outline-2 focus-visible:outline-offset-2";

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
