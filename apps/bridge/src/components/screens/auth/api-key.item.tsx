import { Ellipsis, Key, Trash2 } from "lucide-solid";
import { createMemo, createSignal, Show } from "solid-js";
import { sanitizeScopes } from "../../../helper/data-helper";
import { ApiKeySummary } from "../../../lib/auth";
import ContentTip from "../../ui/content-tip";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuTrigger,
} from "../../ui/dropdown";
import { IconButtonGhost } from "../../ui/icon-btn";
import { Label } from "../../ui/label";
import RemoveKeySheet from "./remove-key-sheet";
import UpdateKeySheet from "./update-key-sheet";

const ApiKeyItem = (props: { apiKey: ApiKeySummary }) => {
  const [openUpdate, setOpenUpdate] = createSignal(false);
  const [openRemove, setOpenRemove] = createSignal(false);

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

  return (
    <div class="flex w-full items-center justify-between px-2 py-1">
      <div class="flex items-center gap-2">
        <Key class="size-3.5 text-neutral-600" />
        <div class="flex flex-col">
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
            <span class="mx-1 opacity-50">•</span>
            <ContentTip
              content={
                <div class="flex flex-col py-1">
                  <Label class="mb-2 text-sm">Allowed Permissions</Label>
                  <div class="flex max-w-50 flex-wrap gap-1">
                    {scopes().map((perm) => (
                      <span class="rounded border bg-neutral-800 px-1 font-mono text-xs text-neutral-400">
                        {perm}
                      </span>
                    ))}
                  </div>
                </div>
              }
            >
              <span class="underline decoration-dotted">
                {isAdmin() ? "Admin" : `${scopes().length} Permissions`}
              </span>
            </ContentTip>
          </div>
        </div>
      </div>
      <DropdownMenu modal={false}>
        <DropdownMenuTrigger as={IconButtonGhost}>
          <Ellipsis class="size-4 text-neutral-500" />
        </DropdownMenuTrigger>
        <DropdownMenuContent>
          <DropdownMenuLabel>Actions</DropdownMenuLabel>
          {/* <DropdownMenuItem onSelect={() => {
                        setTimeout(() => setOpenUpdate(true), 10);
                    }}><KeyRound class="size-3.5" /> Update</DropdownMenuItem> */}
          <DropdownMenuItem
            onSelect={() => {
              setTimeout(() => setOpenRemove(true), 10);
            }}
            class="text-red-400 focus:text-red-400"
          >
            <Trash2 class="size-3.5" /> Remove Key
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      <UpdateKeySheet
        open={openUpdate()}
        onOpenChange={setOpenUpdate}
        apiKey={props.apiKey}
      />
      <RemoveKeySheet
        open={openRemove()}
        onOpenChange={setOpenRemove}
        apiKey={props.apiKey}
      />
    </div>
  );
};

export default ApiKeyItem;
