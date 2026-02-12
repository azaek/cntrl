import { Key } from "lucide-solid";
import { createMemo } from "solid-js";
import { sanitizeScopes } from "../../../helper/data-helper";
import { CreateApiKeyResponse } from "../../../lib/auth";
import CopyBtn from "../../ui/copy-btn";
import { LabelSm } from "../../ui/label";

const NewKeySuccessScreen = ({ apiKey: key }: { apiKey: CreateApiKeyResponse }) => {
  const finalScopes = createMemo(() => {
    return sanitizeScopes(key.record.scopes);
  });

  return (
    <div class="flex flex-col items-center justify-center p-4">
      <div class="relative my-6 flex size-15 items-center justify-center rounded-full border">
        <Key class="z-1" />
        <div class="absolute m-auto size-20 rounded-full border border-dashed" />
        <div class="absolute m-auto size-4 rounded-full bg-blue-400 blur-md"></div>
      </div>
      <p class="text-lg font-medium text-neutral-300">
        {key.record.name} <span class="text-neutral-400">Generated!</span>
      </p>
      <p class="mt-3 text-center text-sm text-balance text-neutral-500">
        Copy this key and keep it safe. You won't be able to see it again.
      </p>
      <div data-corvu-no-drag class="mt-3 flex w-full flex-col items-start">
        <LabelSm class="mb-2 ml-1 uppercase">Key</LabelSm>
        <div class="bg-input flex w-full items-center gap-5 rounded-lg border p-1 px-2 select-text selection:bg-white/20">
          <p class="flex-1 font-mono text-sm break-all text-ellipsis">{key.key}</p>
          <CopyBtn content={key.key} />
        </div>
        <div class="my-2 flex w-full items-center justify-between px-1">
          <LabelSm class="uppercase">Expires</LabelSm>
          <p class="text-sm">
            {key.record.expires_at
              ? new Date(key.record.expires_at).toLocaleDateString()
              : "Never"}
          </p>
        </div>
        <div class="flex w-full items-start justify-between px-1">
          <LabelSm class="uppercase">Permisions</LabelSm>
          <div class="flex max-w-50 flex-wrap justify-end gap-1">
            {finalScopes().map((perm) => (
              <span class="rounded border bg-neutral-800 px-1 font-mono text-xs text-neutral-400">
                {perm}
              </span>
            ))}
          </div>
        </div>
      </div>
    </div>
  );
};

export default NewKeySuccessScreen;
