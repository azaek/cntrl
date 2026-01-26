const Label = (props: { children: string }) => {
  return (
    <p class="text-fg-muted text-[11px] font-medium tracking-widest uppercase">
      {props.children}
    </p>
  );
};

const LabelBlock = (props: { children: string }) => {
  return (
    <div class="flex min-h-7 w-full items-center px-1.5">
      <Label>{props.children}</Label>
    </div>
  );
};

export { Label, LabelBlock };
