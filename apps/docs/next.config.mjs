import { createMDX } from "fumadocs-mdx/next";

const withMDX = createMDX();

/** @type {import('next').NextConfig} */
const config = {
    reactStrictMode: true,
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
