import { BridgesProvider } from "@cntrl-pw/sdk";
import { createFileRoute } from "@tanstack/react-router";
import { useMemo } from "react";
import BridgesControl from "~/components/bridges-contrl";
import { ThemeToggle } from "~/components/theme-toggle";
import { createIndexedDbPersistence } from "~/lib/indexeddb-persistence";

export const Route = createFileRoute("/db")({
  component: DbPage,
});

function DbPage() {
  const persistence = useMemo(() => createIndexedDbPersistence(), []);

  return (
    <BridgesProvider autoConnect={false} persistence={persistence}>
      <div className="flex min-h-svh flex-col items-center justify-center gap-3 p-2">
        <div className="flex flex-col items-center gap-4">
          <h1 className="text-3xl font-bold sm:text-4xl">Cntrl Bridge SDK Test</h1>
          <p className="text-foreground/80 text-sm">
            Custom data layer: <span className="font-mono font-semibold">IndexedDB</span>
          </p>
        </div>
        <BridgesControl />
        <div className="flex flex-col items-center gap-2">
          <ThemeToggle />
        </div>
      </div>
    </BridgesProvider>
  );
}
