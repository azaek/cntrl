import { createFileRoute } from "@tanstack/react-router";
import { ThemeToggle } from "~/components/theme-toggle";

export const Route = createFileRoute("/")({
  component: HomePage,
});

function HomePage() {
  return (
    <div className="flex min-h-svh flex-col items-center justify-center gap-10 p-4">
      <div className="flex flex-col items-center gap-4">
        <h1 className="text-3xl font-bold sm:text-4xl">Cntrl rHub</h1>
        <p className="text-foreground/80 text-center text-sm">
          Raspberry Pi tailored Cntrl Bridge dashboard.
        </p>
      </div>

      <div className="flex flex-col items-center gap-4">
        <p className="text-foreground/60 text-sm">
          Ready for your existing Pi hub migration.
        </p>
        <ThemeToggle />
      </div>
    </div>
  );
}
