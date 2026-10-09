import { type NextRequest, NextResponse } from "next/server";

// Next's own trailing-slash redirect is off (next.config.mjs), since PostHog
// posts to /ingest/e/ and a redirect there loses the event. Every other path
// still loses its trailing slash, as Next did, so each page has one address.
export function proxy(request: NextRequest): NextResponse | undefined {
    const { pathname } = request.nextUrl;
    if (
        pathname.length > 1 &&
        pathname.endsWith("/") &&
        !pathname.startsWith("/ingest/")
    ) {
        // A plain URL: Next's own would put the slash back.
        const url = new URL(request.url);
        url.pathname = pathname.replace(/\/+$/, "");
        return NextResponse.redirect(url.toString(), 308);
    }
    return undefined;
}

export const config = {
    // Not Next's own files, which never end in a slash.
    matcher: ["/((?!_next/).*)"],
};
