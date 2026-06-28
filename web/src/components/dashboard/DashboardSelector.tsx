import type { DashboardListItem } from "@/types/dashboard";

interface DashboardSelectorProps {
  dashboards: DashboardListItem[];
  value: string;
  disabled?: boolean;
  onChange: (nextRef: string) => void;
  label?: string;
  className?: string;
}

export function DashboardSelector({
  dashboards,
  value,
  disabled = false,
  onChange,
  label = "Dashboard",
  className = "min-w-64",
}: DashboardSelectorProps) {
  return (
    <label className="text-sm text-gray-700 flex flex-col gap-1">
      <span>{label}</span>
      <select
        value={value}
        onChange={(event) => onChange(event.target.value)}
        className={`border border-gray-300 rounded px-2 py-1 ${className}`}
        disabled={disabled || dashboards.length === 0}
      >
        {dashboards.map((dashboard) => (
          <option key={dashboard.id} value={dashboard.ref}>
            {dashboard.label} ({dashboard.ref})
          </option>
        ))}
      </select>
    </label>
  );
}
