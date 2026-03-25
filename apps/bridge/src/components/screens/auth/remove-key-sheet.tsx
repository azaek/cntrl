import { Key } from "lucide-solid";
import { Show, createMemo } from "solid-js";
import { useApp } from "../../../context/app-context";
import { sanitizeScopes } from "../../../helper/data-helper";
import { ApiKeySummary, getAuthInfo, removeApiKey } from "../../../lib/auth";
import { Button } from "../../ui/button";
import {
  Drawer,
  DrawerClose,
  DrawerContent,
  DrawerDescription,
  DrawerHeader,
  DrawerTitle,
} from "../../ui/drawer";

const RemoveKeySheet = (props: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  apiKey: ApiKeySummary;
}) => {
  return (
    <Drawer open={props.open} onOpenChange={props.onOpenChange}>
      <DrawerContent>
        <DrawerHeader class="sr-only">
          <DrawerTitle>Remove API Key</DrawerTitle>
          <DrawerDescription>Remove an API key for Cntrl Bridge.</DrawerDescription>
        </DrawerHeader>
        <RemoveKeySheetScreen apiKey={props.apiKey} />
      </DrawerContent>
    </Drawer>
  );
};

const RemoveKeySheetScreen = (props: { apiKey: ApiKeySummary }) => {
  const [_, action] = useApp();

  const scopes = createMemo(() => {
    return sanitizeScopes(props.apiKey.scopes);
  });

  const isAdmin = createMemo(() => {
    return scopes().includes("admin");
  });

  const expires = createMemo(() => {
    if (!props.apiKey.expires_at) return "Never Expires";
    const date = new Date(props.apiKey.expires_at);
    return date.toLocaleDateString();
  });

  const isExpired = createMemo(() => {
    if (!props.apiKey.expires_at) return false;
    const date = new Date(props.apiKey.expires_at);
    return date.getTime() < new Date().getTime();
  });

  const removeKey = async () => {
    await removeApiKey(props.apiKey.id);
    const state = await getAuthInfo();
    action.setAuth(state);
  };

  return (
    <div class="mt-3 flex w-full flex-col px-3 pb-3">
      <p class="text-sm font-semibold">Remove {props.apiKey.name} ?</p>
      <p class="text-muted-foreground text-xs">This action cannot be undone.</p>
      <div class="mt-4 flex flex-col justify-between gap-3 rounded-lg border p-2">
        <div class="flex w-full items-center gap-2 pr-1">
          <Key class="size-4" />
          <div class="flex flex-1 items-center justify-between gap-2">
            <p class="text-sm">
              {props.apiKey.name}{" "}
              <span class="font-mono text-neutral-600">...{props.apiKey.hint}</span>
            </p>
            <div class="flex items-center gap-1.5 text-xs font-bold text-neutral-500">
              <Show
                when={!isExpired()}
                fallback={<span class="text-red-400">Expired</span>}
              >
                <span>{expires()}</span>
              </Show>
            </div>
          </div>
        </div>
        <div class="flex w-full flex-wrap gap-1">
          {scopes().map((perm) => (
            <span class="rounded border bg-neutral-800 px-1 font-mono text-xs text-neutral-400">
              {perm}
            </span>
          ))}
        </div>
      </div>
      <div class="mt-4 flex w-full items-center justify-between">
        <DrawerClose>
          <Button variant="ghost">Cancel</Button>
        </DrawerClose>
        <Button
          variant="destructive"
          onClick={() => {
            removeKey();
          }}
        >
          Remove
        </Button>
      </div>
    </div>
  );
};

export default RemoveKeySheet;
