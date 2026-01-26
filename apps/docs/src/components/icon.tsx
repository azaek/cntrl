import { icons, type LucideProps } from "lucide-react";

export interface IconProps extends LucideProps {
  name: keyof typeof icons;
}

export function Icon({ name, ...props }: IconProps) {
  const LucideIcon = icons[name];
  if (!LucideIcon) {
    return null;
  }
  return <LucideIcon {...props} />;
}
