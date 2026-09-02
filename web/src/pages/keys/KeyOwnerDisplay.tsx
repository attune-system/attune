import { OwnerType } from "@/api";

const ownerLabels: Record<OwnerType, string> = {
  [OwnerType.SYSTEM]: "System",
  [OwnerType.IDENTITY]: "Identity",
  [OwnerType.PACK]: "Pack",
  [OwnerType.ACTION]: "Action",
  [OwnerType.SENSOR]: "Sensor",
};

const ownerBadgeClasses: Record<OwnerType, string> = {
  [OwnerType.SYSTEM]: "bg-purple-100 text-purple-800",
  [OwnerType.IDENTITY]: "bg-blue-100 text-blue-800",
  [OwnerType.PACK]: "bg-green-100 text-green-800",
  [OwnerType.ACTION]: "bg-yellow-100 text-yellow-800",
  [OwnerType.SENSOR]: "bg-indigo-100 text-indigo-800",
};

type KeyOwnerDisplayProps = {
  ownerType: OwnerType;
  ownerRef?: string | null;
};

export default function KeyOwnerDisplay({
  ownerType,
  ownerRef,
}: KeyOwnerDisplayProps) {
  return (
    <div className="flex items-center gap-2">
      <span
        className={`inline-flex rounded-full px-2 py-1 text-xs font-semibold leading-5 ${ownerBadgeClasses[ownerType]}`}
      >
        {ownerLabels[ownerType]}
      </span>
      {ownerType !== OwnerType.SYSTEM && ownerRef && (
        <span className="font-mono text-sm text-gray-900">{ownerRef}</span>
      )}
    </div>
  );
}
