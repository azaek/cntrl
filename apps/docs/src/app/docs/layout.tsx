import { DocsLayout } from "@/components/layout/docs";
import { SidebarBadge } from "@/components/sidebar-badge";
import { baseOptions } from "@/lib/layout.shared";
import { source } from "@/lib/source";
import type * as PageTree from "fumadocs-core/page-tree";
import type { ReactNode } from "react";

export default function Layout({ children }: { children: ReactNode }) {
  const tree = source.getPageTree();
  const pages = source.getPages();
  const badges = new Map(pages.map((page) => [page.url, page.data.badge]));

  const transform = (node: PageTree.Node): PageTree.Node => {
    if (node.type === "page" && badges.has(node.url)) {
      return { ...node, badge: badges.get(node.url) } as PageTree.Node;
    }

    if (node.type === "folder") {
      return { ...node, children: node.children.map(transform) };
    }

    return node;
  };

  const modifiedTree = {
    ...tree,
    children: tree.children.map(transform),
  };

  return (
    <DocsLayout
      tree={modifiedTree}
      {...baseOptions()}
      sidebar={{
        components: {
          Item: SidebarBadge,
        },
      }}
    >
      {children}
    </DocsLayout>
  );
}
