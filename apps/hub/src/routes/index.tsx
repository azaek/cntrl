import { createFileRoute } from "@tanstack/react-router";
import { ThemeToggle } from "~/components/theme-toggle";

export const Route = createFileRoute("/")({
  component: HomePage,
});

function HomePage() {
  return (
    <div className="flex min-h-svh flex-col items-center justify-center gap-10 p-4">
      <div className="flex flex-col items-center gap-4">
        <h1 className="text-3xl font-bold sm:text-4xl">Cntrl Hub</h1>
        <p className="text-foreground/80 text-center text-sm">
          Standalone dashboard for your Cntrl Bridge devices.
        </p>
      </div>

      <div className="flex flex-col items-center gap-4">
        <p className="text-foreground/60 text-sm">
          Configure your bridge connection to get started.
        </p>
        <ThemeToggle />
      </div>
    </div>
  );
}
