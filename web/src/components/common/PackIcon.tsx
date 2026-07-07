import { useEffect, useState } from "react";
import { Cog } from "lucide-react";
import { loadPackIconSrc } from "@/utils/packIcons";

type PackIconProps = {
  packRef?: string | null;
  size?: "xs" | "sm" | "md" | "lg";
  className?: string;
  title?: string;
};

const SIZE_CLASSES = {
  xs: "h-4 w-4",
  sm: "h-5 w-5",
  md: "h-8 w-8",
  lg: "h-10 w-10",
};

const FALLBACK_ICON_CLASSES = {
  xs: "h-3 w-3",
  sm: "h-3.5 w-3.5",
  md: "h-5 w-5",
  lg: "h-6 w-6",
};

export default function PackIcon({
  packRef,
  size = "sm",
  className = "",
  title,
}: PackIconProps) {
  const [src, setSrc] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;

    if (!packRef) {
      setSrc(null);
      return () => {
        cancelled = true;
      };
    }

    void loadPackIconSrc(packRef).then((resolvedSrc) => {
      if (!cancelled) {
        setSrc(resolvedSrc);
      }
    });

    return () => {
      cancelled = true;
    };
  }, [packRef]);

  const wrapperClass = `${SIZE_CLASSES[size]} ${className} inline-flex flex-shrink-0 items-center justify-center overflow-hidden rounded bg-gray-100 text-gray-500`;

  if (!src) {
    return (
      <span className={wrapperClass} title={title || packRef || "Pack icon"}>
        <Cog className={FALLBACK_ICON_CLASSES[size]} />
      </span>
    );
  }

  return (
    <span className={wrapperClass} title={title || packRef || "Pack icon"}>
      <img
        src={src}
        alt=""
        className="h-full w-full object-contain"
      />
    </span>
  );
}
