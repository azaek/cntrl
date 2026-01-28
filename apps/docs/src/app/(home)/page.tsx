import Link from "fumadocs-core/link";
import {
  Activity,
  BookOpen,
  Download,
  FishingHook,
  Headphones,
  Power,
  Shield,
  Zap,
} from "lucide-react";
import Image from "next/image";
import CopyBtn from "./copybtn";

export default function HomePage() {
  return (
    <main className="font-sg flex min-h-[calc(100vh-57px)] flex-1 flex-col items-center">
      <div className="bg-fd-background relative flex h-10 w-full max-w-350 flex-1 items-stretch overflow-hidden min-[1400px]:border-x">
        <div
          className="pointer-events-none absolute top-20 left-0 hidden aspect-656/792 w-full max-w-164 xl:block"
          style={{
            maskImage: "linear-gradient(to bottom,white 70%,transparent)",
          }}
        >
          <Image
            src="/mac-lg.png"
            alt="Bridge Banner"
            width={656}
            height={792}
            style={{
              objectFit: "contain",
            }}
            className="aspect-656/792 size-full"
          />
        </div>
        <div
          className="pointer-events-none absolute top-20 left-0 hidden aspect-564/819 w-full max-w-141 max-[1100px]:translate-x-[-10%] lg:block xl:hidden"
          style={{
            maskImage: "linear-gradient(to bottom,white 70%,transparent)",
          }}
        >
          <Image
            src="/mac-md.png"
            alt="Bridge Banner"
            width={564}
            height={819}
            style={{
              objectFit: "contain",
            }}
            className="aspect-564/819 size-full"
          />
        </div>

        <div className="hidden h-40 w-124 flex-col max-[1100px]:w-110 lg:flex xl:w-145"></div>
        <div className="flex w-full flex-1 flex-col items-start pr-6 pl-6 lg:pl-0">
          <div className="pointer-events-none relative -mx-6 flex h-50 w-full sm:h-32.5">
            <Image
              src="/mac-xs.png"
              alt="Bridge Banner"
              width={402}
              height={383}
              style={{
                objectFit: "contain",
                maskImage: "linear-gradient(to bottom,white 10%,transparent 60%)",
              }}
              className="absolute -top-5 mx-auto aspect-402/383 w-screen max-w-100.5 sm:hidden"
            />
          </div>
          <div className="flex w-full items-stretch">
            <div className="pointer-events-none relative hidden w-48 items-start sm:flex lg:hidden">
              <Image
                src="/mac-sm.png"
                alt="Bridge Banner"
                width={256}
                height={556}
                style={{
                  objectFit: "contain",
                }}
                className="absolute -left-12 min-w-[256px] translate-y-[-10%] sm:-left-6"
              />
            </div>
            <div className="flex-1">
              <h1 className="text-fd-foreground/80 text-2xl leading-[1] font-bold text-balance sm:text-3xl md:text-[40px] xl:text-5xl">
                Bridge the gap between your desktop & web apps
              </h1>
              <p className="text-fd-foreground/60 mt-10 text-sm text-balance sm:text-sm md:text-base">
                Native desktop app that exposes your Windows or Mac system controls, and
                real-time data through a REST API. Build control dashboards with our React
                SDK.
              </p>
              <div className="mt-10 flex flex-wrap items-center gap-3">
                <Link href="https://github.com/azaek/cntrl/releases" external>
                  <button className="bg-fd-accent text-fd-foreground hover:bg-fd-accent/80 flex min-h-10 cursor-pointer items-center gap-2 rounded-md px-3 py-1 sm:px-6">
                    <Download className="size-4" />
                    <p className="text-sm leading-none font-medium">Download Bridge</p>
                  </button>
                </Link>
                <Link href="/docs">
                  <button className="text-fd-foreground hover:bg-fd-card/80 flex min-h-10 cursor-pointer items-center gap-2 rounded-md border px-3 py-1 sm:px-6">
                    <BookOpen className="size-4" />
                    <p className="text-sm leading-none font-medium">Documentation</p>
                  </button>
                </Link>
              </div>
              <p className="text-fd-foreground/30 mt-1 text-xs font-medium">
                Available for Windows & Mac
              </p>
              <div className="mt-10 flex flex-col items-start gap-1">
                <p className="text-fd-foreground/40 text-xs">
                  React SDK available via npm
                </p>
                <div className="flex min-h-10 min-w-[80%] items-center gap-2 rounded-md border px-2.5 py-1.5 sm:min-w-95">
                  <Link href="https://www.npmjs.com/package/@cntrl-pw/sdk" external>
                    <svg
                      width="24"
                      height="24"
                      viewBox="0 0 24 24"
                      fill="none"
                      xmlns="http://www.w3.org/2000/svg"
                    >
                      <path
                        d="M20.002 3C20.5543 3 21.002 3.44772 21.002 4V20C21.002 20.5523 20.5543 21 20.002 21H4.00195C3.44966 21 3.00195 20.5523 3.00195 20V4C3.00195 3.44772 3.44966 3 4.00195 3H20.002ZM17.002 7H7.00195V17H12.002V9.5H14.502V17H17.002V7Z"
                        fill="#9E9E9E"
                      />
                    </svg>
                  </Link>
                  <p className="flex-1 font-mono text-sm font-bold">
                    npm i @cntrl-pw/sdk
                  </p>
                  <CopyBtn text="npm i @cntrl-pw/sdk" />
                </div>
              </div>
            </div>
          </div>
          <div className="max-h-21.25 flex-1"></div>
          <div className="mt-20 flex w-full flex-col items-start sm:max-w-152.5 sm:self-center lg:max-w-[unset] lg:self-auto">
            <h2 className="text-fd-foreground/80 text-sm">Features</h2>
            <div className="mt-4 grid w-full grid-cols-1 gap-5 p-2 sm:grid-cols-3">
              <FeatureCard
                Icon={Power}
                title="Power Controls"
                description="Shutdown, restart, sleep, hibernate your machines"
              />
              <FeatureCard
                Icon={Activity}
                title="System Stats"
                description="Real-time CPU, memory, disk, network monitoring"
              />
              <FeatureCard
                Icon={Headphones}
                title="Media Controls"
                description="Control media playback and volume with updates"
              />
              <FeatureCard
                Icon={Shield}
                title="Authentication"
                description="API key authentication with IP whitelist/blacklist controls"
              />
              <FeatureCard
                Icon={Zap}
                title="WebSocket"
                description="Real-time updates and bi-directional communication"
              />
              <FeatureCard
                Icon={FishingHook}
                title="React Hooks"
                description="Simple, type-safe API for seamless integration"
              />
            </div>
          </div>
          <div className="min-h-20 flex-1"></div>
          <div className="my-4 flex items-center gap-2 sm:self-center lg:self-auto">
            <p className="text-fd-muted-foreground/40 text-xs">Copyright © 2026 Cntrl</p>
          </div>
        </div>
      </div>
    </main>
  );
}

const FeatureCard = ({
  Icon,
  title,
  description,
}: {
  Icon: typeof Power;
  title: string;
  description: string;
}) => {
  return (
    <div className="flex flex-col items-start gap-2 py-1">
      <div className="text-fd-muted-foreground flex items-center gap-1">
        <Icon className="size-4" />
        <h3 className="text-xs font-medium">{title}</h3>
      </div>
      <h4 className="text-fd-secondary-foreground/80 text-xs text-balance">
        {description}
      </h4>
    </div>
  );
};
