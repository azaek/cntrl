"use client";

import { Check, Copy } from "lucide-react";
import { useState } from "react";

const CopyBtn = ({ text }: { text: string }) => {
  const [copied, setCopied] = useState(false);

  const copy = () => {
    navigator.clipboard.writeText(text);
    setCopied(true);
    setTimeout(() => {
      setCopied(false);
    }, 2000);
  };

  return (
    <button
      className="hover:bg-fd-card/80 flex size-6 cursor-pointer items-center justify-center rounded"
      onClick={copy}
    >
      {copied ? <Check className="size-4" /> : <Copy className="size-4" />}
    </button>
  );
};

export default CopyBtn;
