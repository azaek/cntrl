import type { Metadata } from "next/types";
import { Page } from "./source";

export function createMetadata(override: Metadata): Metadata {
  return {
    ...override,
    openGraph: {
      title: override.title ?? undefined,
      description: override.description ?? undefined,
      url: "https://www.cntrl.pw",
      images: "/bridge-banner.png",
      siteName: "Cntrl",
      ...override.openGraph,
    },
    twitter: {
      card: "summary_large_image",
      creator: "@datbugdied",
      title: override.title ?? undefined,
      description: override.description ?? undefined,
      images: "/bridge-banner.png",
      ...override.twitter,
    },
    // alternates: {
    //   types: {
    //     'application/rss+xml': [
    //       {
    //         title: 'Cntrl Blog',
    //         url: 'https://cntrl.azaek.dev/blog/rss.xml',
    //       },
    //     ],
    //   },
    //   ...override.alternates,
    // },
  };
}

export function getPageImage(page: Page) {
  const segments = [...page.slugs, "image.webp"];

  return {
    segments,
    url: `/og/${segments.join("/")}`,
  };
}

export const baseUrl =
  process.env.NODE_ENV === "development" || !process.env.VERCEL_PROJECT_PRODUCTION_URL
    ? new URL("http://localhost:3000")
    : new URL(`https://${process.env.VERCEL_PROJECT_PRODUCTION_URL}`);
