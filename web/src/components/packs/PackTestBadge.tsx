import { CheckCircle, XCircle, Clock, AlertCircle } from "lucide-react";

interface PackTestBadgeProps {
  status: string;
  passed?: number;
  total?: number;
  size?: "sm" | "md" | "lg";
  showCounts?: boolean;
}

export default function PackTestBadge({
  status,
  passed,
  total,
  size = "md",
  showCounts = true,
}: PackTestBadgeProps) {
  const getStatusConfig = () => {
    switch (status) {
      case "passed":
        return {
          icon: CheckCircle,
          text: "Passed",
          bgColor: "bg-green-50",
          textColor: "text-green-700",
          borderColor: "border-green-200",
          iconColor: "text-green-600",
        };
      case "failed":
        return {
          icon: XCircle,
          text: "Failed",
          bgColor: "bg-red-50",
          textColor: "text-red-700",
          borderColor: "border-red-200",
          iconColor: "text-red-600",
        };
      case "skipped":
        return {
          icon: Clock,
          text: "Skipped",
          bgColor: "bg-gray-50",
          textColor: "text-gray-700",
          borderColor: "border-gray-200",
          iconColor: "text-gray-600",
        };
      default:
        return {
          icon: AlertCircle,
          text: "Unknown",
          bgColor: "bg-yellow-50",
          textColor: "text-yellow-700",
          borderColor: "border-yellow-200",
          iconColor: "text-yellow-600",
        };
    }
  };

  const getSizeClasses = () => {
    switch (size) {
      case "sm":
        return {
          container: "px-2 py-1 text-xs",
          icon: "w-3 h-3",
          gap: "gap-1",
        };
      case "lg":
        return {
          container: "px-4 py-2 text-base",
          icon: "w-5 h-5",
          gap: "gap-2",
        };
      default:
        return {
          container: "px-3 py-1.5 text-sm",
          icon: "w-4 h-4",
          gap: "gap-1.5",
        };
    }
  };

  const config = getStatusConfig();
  const sizeClasses = getSizeClasses();
  const Icon = config.icon;

  return (
    <span
      className={`inline-flex items-center ${sizeClasses.gap} ${sizeClasses.container} ${config.bgColor} ${config.textColor} border ${config.borderColor} rounded-full font-medium`}
    >
      <Icon className={`${sizeClasses.icon} ${config.iconColor}`} />
      <span>{config.text}</span>
      {showCounts && passed !== undefined && total !== undefined && (
        <span className="font-semibold">
          {passed}/{total}
        </span>
      )}
    </span>
  );
}
