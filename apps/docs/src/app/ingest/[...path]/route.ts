// /ingest: the page's analytics events, passed to PostHog, as Console's are
// (its D105). The request is rebuilt from a few headers, never this site's
// cookies or the visitor's IP, and only PostHog's event paths pass.

/** PostHog's paths for events: single, batched, and the newer `/i/v0/e/`. */
const PATHS = /^(e|batch|i\/v0\/e)\/?$/;

/** The headers PostHog needs to read an event, and nothing else. */
const HEADERS = ["content-type", "content-encoding", "user-agent"];

const HOST = "https://us.i.posthog.com";

export async function POST(request: Request): Promise<Response> {
    const incoming = new URL(request.url);
    // The path as the page sent it, trailing slash and all: `/ingest/e/` is PostHog's `/e/`.
    const path = incoming.pathname.replace(/^\/ingest\//, "");
    if (!process.env.NEXT_PUBLIC_POSTHOG_KEY || !PATHS.test(path)) {
        return new Response(null, { status: 404 });
    }
    const headers = new Headers();
    for (const name of HEADERS) {
        const value = request.headers.get(name);
        if (value) headers.set(name, value);
    }
    const upstream = new URL(`/${path}`, HOST);
    upstream.search = incoming.search;
    try {
        const response = await fetch(upstream, {
            method: "POST",
            headers,
            body: await request.arrayBuffer(),
        });
        // Nothing of PostHog's sets anything on this site's domain.
        return new Response(response.body, {
            status: response.status,
            headers: {
                "content-type":
                    response.headers.get("content-type") ?? "application/json",
            },
        });
    } catch {
        return new Response(null, { status: 502 });
    }
}
