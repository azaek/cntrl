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
    alternates: {
      canonical: override.alternates?.canonical ?? undefined,
      ...override.alternates,
    },
  };
}

export function createSoftwareAppJsonLd() {
  return {
    "@context": "https://schema.org",
    "@type": "SoftwareApplication",
    name: "Cntrl Bridge",
    applicationCategory: "UtilitiesApplication",
    operatingSystem: "Windows, macOS",
    description:
      "Native desktop app that exposes your system controls and real-time data through a REST API.",
    url: "https://www.cntrl.pw",
    offers: {
      "@type": "Offer",
      price: "0",
      priceCurrency: "USD",
    },
  };
}

export function createArticleJsonLd(page: Page) {
  return {
    "@context": "https://schema.org",
    "@type": "TechArticle",
    headline: page.data.title,
    description: page.data.description ?? "",
    url: `https://www.cntrl.pw${page.url}`,
    publisher: {
      "@type": "Organization",
      name: "Cntrl",
      url: "https://www.cntrl.pw",
    },
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
