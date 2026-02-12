import { Key } from "lucide-solid";
import { createMemo, Show } from "solid-js";
import { sanitizeScopes } from "../../../helper/data-helper";
import { ApiKeySummary } from "../../../lib/auth";
import ContentTip from "../../ui/content-tip";
import { Label } from "../../ui/label";

const ApiKeyItem = ({ apiKey: key }: { apiKey: ApiKeySummary }) => {
  const scopes = createMemo(() => {
    return sanitizeScopes(key.scopes);
  });

  const isAdmin = createMemo(() => {
    return scopes().includes("admin");
  });

  const expires = createMemo(() => {
    if (!key.expires_at) return "Never Expires";
    const date = new Date(key.expires_at);
    return date.toLocaleDateString();
  });

  const isExpired = createMemo(() => {
    if (!key.expires_at) return false;
    const date = new Date(key.expires_at);
    return date.getTime() < new Date().getTime();
  });

  return (
    <div class="flex w-full items-center justify-between px-2 py-1">
      <div class="flex items-center gap-2">
        <Key class="size-3.5 text-neutral-600" />
        <div class="flex flex-col">
          <p class="text-sm">
            {key.name} <span class="font-mono text-neutral-600">...{key.hint}</span>
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
      <p class="text-sm">{key.hint}</p>
    </div>
  );
};

export default ApiKeyItem;
