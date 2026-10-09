import { LLMCopyButton, ViewOptions } from "@/components/ai/page-actions";
import { ConsoleCard } from "@/components/console-card";
import {
    DocsBody,
    DocsDescription,
    DocsPage,
    DocsTitle,
} from "@/components/layout/docs/page";
import { createArticleJsonLd } from "@/lib/metadata";
import { getPageImage, source } from "@/lib/source";
import { getMDXComponents } from "@/mdx-components";
import { createRelativeLink } from "fumadocs-ui/mdx";
import type { Metadata } from "next";
import { notFound } from "next/navigation";

export default async function Page(props: PageProps<"/docs/[[...slug]]">) {
    const params = await props.params;
    const page = source.getPage(params.slug);
    if (!page) notFound();

    const MDX = page.data.body;
    const gitConfig = {
        user: "azaek",
        repo: "cntrl",
        branch: "main",
    };

    return (
        <DocsPage
            tableOfContent={{
                style: "clerk",
                single: true,
                // Only beside headings, since a footer alone would show an empty "On this
                // page", and not in Console's own docs, whose readers know Console already.
                footer:
                    page.data.toc.length > 0 && params.slug?.[0] !== "console" ? (
                        <ConsoleCard size="toc" />
                    ) : undefined,
            }}
            toc={page.data.toc}
            full={page.data.full}
        >
            <script
                type="application/ld+json"
                dangerouslySetInnerHTML={{
                    __html: JSON.stringify(createArticleJsonLd(page)),
                }}
            />
            <DocsTitle>{page.data.title}</DocsTitle>
            <DocsDescription className="mb-0">{page.data.description}</DocsDescription>
            <div className="flex flex-row items-center gap-2 border-b pb-6">
                <LLMCopyButton markdownUrl={`${page.url}.mdx`} />
                <ViewOptions
                    markdownUrl={`${page.url}.mdx`}
                    // update it to match your repo
                    githubUrl={`https://github.com/${gitConfig.user}/${gitConfig.repo}/blob/${gitConfig.branch}/docs/content/docs/${page.path}`}
                />
            </div>
            <DocsBody>
                <MDX
                    components={getMDXComponents({
                        // this allows you to link to other pages with relative file paths
                        a: createRelativeLink(source, page),
                    })}
                />
            </DocsBody>
        </DocsPage>
    );
}

export async function generateStaticParams() {
    return source.generateParams();
}

export async function generateMetadata(
    props: PageProps<"/docs/[[...slug]]">,
): Promise<Metadata> {
    const params = await props.params;
    const page = source.getPage(params.slug);
    if (!page) notFound();

    return {
        title: page.data.title,
        description: page.data.description,
        openGraph: {
            images: getPageImage(page).url,
        },
    };
}
