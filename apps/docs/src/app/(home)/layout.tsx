import { HomeLayout } from "@/components/layout/home";
import { baseOptions, linkItems } from "@/lib/layout.shared";
import Link from "fumadocs-core/link";
import {
  NavbarMenu,
  NavbarMenuContent,
  NavbarMenuLink,
  NavbarMenuTrigger,
} from "fumadocs-ui/layouts/home/navbar";
import { Package, Radio, Shield } from "lucide-react";
import Image from "next/image";

export default function Layout({ children }: LayoutProps<"/">) {
  return (
    <HomeLayout
      {...baseOptions()}
      links={[
        {
          type: "menu",
          on: "menu",
          text: "Documentation",
          items: [
            {
              text: "Cntrl Bridge",
              url: "/docs",
              icon: <Radio />,
            },
            {
              text: "React SDK",
              url: "/docs/sdk",
              icon: <Package />,
            },
          ],
        },
        {
          type: "custom",
          on: "nav",
          children: (
            <NavbarMenu>
              <NavbarMenuTrigger>
                <Link href="/docs">Documentation</Link>
              </NavbarMenuTrigger>
              <NavbarMenuContent>
                <NavbarMenuLink href="/docs" className="md:row-span-2">
                  <div className="-mx-3 -mt-3">
                    <Image
                      src={"/banner-menu.png"}
                      alt="Perview"
                      width={1200}
                      height={630}
                      className="rounded-t-lg object-cover"
                      // style={{
                      //   maskImage: 'linear-gradient(to bottom,white 70%,transparent)',
                      // }}
                    />
                  </div>
                  <p className="font-medium">Getting Started with Bridge</p>
                  <p className="text-fd-muted-foreground text-sm">
                    Learn to use Cntrl Bridge on your system.
                  </p>
                </NavbarMenuLink>
                <NavbarMenuLink href="/docs" className="md:row-span-2">
                  <div className="-mx-3 -mt-3">
                    <Image
                      src={"/banner-menu.png"}
                      alt="Perview"
                      width={1200}
                      height={630}
                      className="rounded-t-lg object-cover"
                      // style={{
                      //   maskImage: 'linear-gradient(to bottom,white 70%,transparent)',
                      // }}
                    />
                  </div>
                  <p className="font-medium">React SDK</p>
                  <p className="text-fd-muted-foreground text-sm">
                    Consume bridge in your React app using hooks.
                  </p>
                </NavbarMenuLink>

                <NavbarMenuLink href="/docs/sdk" className="lg:col-span-1">
                  <Shield className="bg-fd-primary text-fd-primary-foreground mb-2 rounded-md p-1" />
                  <p className="font-medium">Authentication</p>
                  <p className="text-fd-muted-foreground text-sm">
                    Manage bridge authentication.
                  </p>
                </NavbarMenuLink>

                <NavbarMenuLink href="/docs/openapi" className="lg:col-span-1">
                  <Shield className="bg-fd-primary text-fd-primary-foreground mb-2 rounded-md p-1" />
                  <p className="font-medium">UI Widgets</p>
                  <p className="text-fd-muted-foreground text-sm">
                    Premade UI widgets using bridge SDK.
                  </p>
                </NavbarMenuLink>
              </NavbarMenuContent>
            </NavbarMenu>
          ),
        },
        ...linkItems,
      ]}
    >
      {children}
    </HomeLayout>
  );
}
