import { PostHogProvider } from "@/components/posthog-provider";
import { createMetadata } from "@/lib/metadata";
import { RootProvider } from "fumadocs-ui/provider/next";
import { Inter, Space_Grotesk } from "next/font/google";
import "./global.css";

const inter = Inter({
  subsets: ["latin"],
  variable: "--font-inter",
});

const spaceGrotesk = Space_Grotesk({
  subsets: ["latin"],
  variable: "--font-sg",
});

export const metadata = createMetadata({
  title: {
    template: "%s | Cntrl",
    default: "Cntrl",
  },
  icons: {
    icon: [
      {
        url: "/favicon-light.png",
        rel: "icon",
        media: "(prefers-color-scheme: light)",
      },
      {
        url: "/favicon.png",
        rel: "icon",
        media: "(prefers-color-scheme: dark)",
        sizes: "16x16",
      },
    ],
  },
  description: "Cntrl is a remote management bridge for your local machines.",
  metadataBase:
    process.env.NODE_ENV === "development"
      ? new URL("http://localhost:3003")
      : new URL("https://www.cntrl.pw"),
  keywords: [
    "cntrl",
    "remote desktop",
    "system monitoring",
    "REST API",
    "WebSocket",
    "React SDK",
    "local network",
    "desktop bridge",
  ],
});

export default function Layout({ children }: LayoutProps<"/">) {
  return (
    <html
      lang="en"
      className={spaceGrotesk.variable + " " + inter.className}
      suppressHydrationWarning
    >
      <body className="flex min-h-screen flex-col">
        <PostHogProvider>
          <RootProvider>{children}</RootProvider>
        </PostHogProvider>
      </body>
    </html>
  );
}
