import { createMetadata } from "@/lib/metadata";
import { RootProvider } from "fumadocs-ui/provider/next";
import { Inter } from "next/font/google";
import "./global.css";

const inter = Inter({
  subsets: ["latin"],
});

export const metadata = createMetadata({
  title: {
    template: "%s | Cntrl",
    default: "Cntrl",
  },
  description: "Cntrl is a remote management bridge for your local machines.",
  metadataBase:
    process.env.NODE_ENV === "development"
      ? new URL("http://localhost:3001")
      : new URL("https://cntrl.azaek.dev"),
});

export default function Layout({ children }: LayoutProps<"/">) {
  return (
    <html lang="en" className={inter.className} suppressHydrationWarning>
      <body className="flex min-h-screen flex-col">
        <RootProvider>{children}</RootProvider>
      </body>
    </html>
  );
}
