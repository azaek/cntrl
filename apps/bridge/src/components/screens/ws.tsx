import { ChevronLeft, ChevronsLeftRightEllipsis } from "lucide-solid";
import { useApp } from "../../context/app-context";
import { Button } from "../ui/button";
import Container from "../ui/container";
import Divider from "../ui/divider";

const isValidIp = (ip: string) => {
  const parts = ip.split(".");
  if (parts.length !== 4) return false;
  return parts.every((part) => {
    const num = parseInt(part);
    return !isNaN(num) && num >= 0 && num <= 255;
  });
};

const WsScreen = () => {
  const [store, actions] = useApp();

  return (
    <>
      <Container className="max-h-106.5">
        <div class="bg-bg sticky top-0 z-10 flex w-full flex-col gap-0.5">
          <div class="flex w-full items-center gap-2 rounded px-2 py-1.5">
            <Button
              onClick={() => actions.setPage("main")}
              className="-mx-1 size-9! w-auto flex-[unset] items-center justify-center p-0!"
            >
              <ChevronLeft class="text-fg-muted size-4.5" />
            </Button>
            <ChevronsLeftRightEllipsis class="size-4.5" />
            <div class="flex flex-1 flex-col items-start">
              <p class="text-sm font-medium text-white">Websocket Config</p>
              <p class="text-fg-muted text-xs font-medium">
                Mange websocket loop intervals
              </p>
            </div>
          </div>
          <Divider />
        </div>
      </Container>
    </>
  );
};

export default WsScreen;
