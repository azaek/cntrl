import { createMDX } from "fumadocs-mdx/next";

const withMDX = createMDX();

/** @type {import('next').NextConfig} */
const config = {
    reactStrictMode: true,
    // PostHog posts to /ingest/e/, which Next would redirect, losing the
    // event; src/proxy.ts drops other paths' trailing slashes instead.
    skipTrailingSlashRedirect: true,
    images: {
        remotePatterns: [
            {
                protocol: "https",
                hostname: "github.com",
            },
            {
                protocol: "https",
                hostname: "raw.githubusercontent.com",
            },
        ],
    },
    // Short addresses for the legal pages, as Console, Polar and SignPath link them.
    async redirects() {
        return [
            { source: "/privacy", destination: "/docs/legal/privacy", permanent: false },
            { source: "/terms", destination: "/docs/legal/terms", permanent: false },
            // The Console docs were the "Cntrl Hub" section before Console launched.
            {
                source: "/docs/hub/:path*",
                destination: "/docs/console/:path*",
                permanent: false,
            },
        ];
    },
    async rewrites() {
        return [
            {
                source: "/docs/:path*.mdx",
                destination: "/llms.mdx/docs/:path*",
            },
        ];
    },
};

export default withMDX(config);
