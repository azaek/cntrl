import { HomeLayout } from "@/components/layout/home";
import { baseOptions, linkItems } from "@/lib/layout.shared";
import Link from "fumadocs-core/link";
import {
  NavbarMenu,
  NavbarMenuContent,
  NavbarMenuLink,
  NavbarMenuTrigger,
} from "fumadocs-ui/layouts/home/navbar";
import { Package, Radio, Settings2, Shield } from "lucide-react";
import Image from "next/image";

export default function Layout({ children }: LayoutProps<"/">) {
  return (
    <HomeLayout
      {...baseOptions()}
      searchToggle={{
        enabled: false,
      }}
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
            {
              text: "Authentication",
              url: "/docs/auth",
              icon: <Shield />,
            },
            {
              text: "Configuration",
              url: "/docs/config",
              icon: <Settings2 />,
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
                      src={"/bridge-banner.png"}
                      alt="Perview"
                      width={1200}
                      height={630}
                      className="hidden rounded-t-lg object-cover dark:block"
                      style={{
                        maskImage: "linear-gradient(to bottom,white 70%,transparent)",
                      }}
                    />
                    <Image
                      src={"/bridge-banner-light.png"}
                      alt="Perview"
                      width={1200}
                      height={630}
                      className="block rounded-t-lg object-cover dark:hidden"
                      style={{
                        maskImage: "linear-gradient(to bottom,white 70%,transparent)",
                      }}
                    />
                  </div>
                  <p className="font-medium">Getting Started with Bridge</p>
                  <p className="text-fd-muted-foreground text-sm">
                    Learn to use Cntrl Bridge on your system.
                  </p>
                </NavbarMenuLink>
                <NavbarMenuLink href="/docs/sdk" className="md:row-span-2">
                  <div className="-mx-3 -mt-3">
                    <Image
                      src={"/sdk-banner-m.png"}
                      alt="Perview"
                      width={1200}
                      height={630}
                      className="hidden rounded-t-lg object-cover dark:block"
                      style={{
                        maskImage: "linear-gradient(to bottom,white 70%,transparent)",
                      }}
                    />
                    <Image
                      src={"/sdk-banner-m-light.png"}
                      alt="Perview"
                      width={1200}
                      height={630}
                      className="block rounded-t-lg object-cover dark:hidden"
                      style={{
                        maskImage: "linear-gradient(to bottom,white 70%,transparent)",
                      }}
                    />
                  </div>
                  <p className="font-medium">React SDK</p>
                  <p className="text-fd-muted-foreground text-sm">
                    Consume bridge in your React app using hooks.
                  </p>
                </NavbarMenuLink>

                <NavbarMenuLink href="/docs/auth" className="lg:col-span-1">
                  <Shield className="text-fd-foreground mb-2 size-6! rounded-md p-1" />
                  <div className="flex-1" />
                  <p className="font-medium">Authentication</p>
                  <p className="text-fd-muted-foreground text-sm">
                    Manage bridge authentication.
                  </p>
                </NavbarMenuLink>

                <NavbarMenuLink href="/docs/config" className="lg:col-span-1">
                  <Settings2 className="text-fd-foreground mb-2 size-6! rounded-md p-1" />
                  <div className="flex-1" />
                  <p className="font-medium">Configuration</p>
                  <p className="text-fd-muted-foreground text-sm">
                    Configure bridge settings.
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
