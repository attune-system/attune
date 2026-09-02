import { useId, useMemo } from "react";
import { OwnerType } from "@/api";
import { usePacks } from "@/hooks/usePacks";
import { useActions } from "@/hooks/useActions";
import { useSensors } from "@/hooks/useSensors";
import { useAuth } from "@/contexts/AuthContext";
import SearchableSelect from "@/components/common/SearchableSelect";
import { ownerTypeLabel } from "@/components/caches/cacheUtils";

export interface OwnerScopeValue {
  ownerType: OwnerType;
  /** Denormalized ref for pack/action/sensor. Ignored for system/identity. */
  ownerRef: string;
}

interface OwnerScopeSelectorCommonProps {
  disabled?: boolean;
  /** Restricts which owner types are selectable (defaults to all five). */
  allowedOwnerTypes?: OwnerType[];
  ownerTypeLabelText?: string;
  ownerRefLabelText?: string;
}

type OwnerScopeSelectorProps = OwnerScopeSelectorCommonProps &
  (
    | {
        includeAny: true;
        value: OwnerScopeValue | null;
        onChange: (value: OwnerScopeValue | null) => void;
      }
    | {
        includeAny?: false;
        value: OwnerScopeValue;
        onChange: (value: OwnerScopeValue) => void;
      }
  );

const ALL_OWNER_TYPES = [
  OwnerType.SYSTEM,
  OwnerType.IDENTITY,
  OwnerType.PACK,
  OwnerType.ACTION,
  OwnerType.SENSOR,
];

/**
 * Owner-type + owner-ref picker shared by the namespace index filters and the
 * create-namespace form. Namespaces use the same owner scoping as Keys, so
 * this mirrors that owner-type selection UX while resolving actual pack/
 * action/sensor refs from their respective list endpoints instead of free
 * text, to reduce owner-ref typos that would otherwise silently resolve to
 * "not found" once the real cache API performs canonical owner resolution.
 */
export default function OwnerScopeSelector(props: OwnerScopeSelectorProps) {
  const {
    value,
    disabled = false,
    allowedOwnerTypes = ALL_OWNER_TYPES,
    ownerTypeLabelText = "Owner scope",
    ownerRefLabelText = "Owner reference",
  } = props;
  const ownerTypeSelectId = useId();
  const { user } = useAuth();
  const { data: packsData, isLoading: packsLoading } = usePacks({
    pageSize: 1000,
  });
  const { data: actionsData, isLoading: actionsLoading } = useActions({
    pageSize: 1000,
  });
  const { data: sensorsData, isLoading: sensorsLoading } = useSensors({
    pageSize: 1000,
  });

  const packOptions = useMemo(
    () =>
      (packsData?.items ?? [])
        .map((pack) => ({ value: pack.ref, label: pack.ref }))
        .sort((a, b) => a.label.localeCompare(b.label)),
    [packsData?.items],
  );
  const actionOptions = useMemo(
    () =>
      (actionsData?.items ?? [])
        .map((action) => ({ value: action.ref, label: action.ref }))
        .sort((a, b) => a.label.localeCompare(b.label)),
    [actionsData?.items],
  );
  const sensorOptions = useMemo(
    () =>
      (sensorsData?.items ?? [])
        .map((sensor) => ({ value: sensor.ref, label: sensor.ref }))
        .sort((a, b) => a.label.localeCompare(b.label)),
    [sensorsData?.items],
  );

  const handleOwnerTypeChange = (nextType: OwnerType | "") => {
    if (nextType === "") {
      if (props.includeAny) {
        props.onChange(null);
      }
      return;
    }
    const next = { ownerType: nextType, ownerRef: "" };
    if (props.includeAny) {
      props.onChange(next);
    } else {
      props.onChange(next);
    }
  };

  return (
    <div className="grid gap-4 sm:grid-cols-2">
      <div>
        <label
          htmlFor={ownerTypeSelectId}
          className="block text-xs font-medium uppercase tracking-wide text-gray-500"
        >
          {ownerTypeLabelText}
        </label>
        <select
          id={ownerTypeSelectId}
          value={value?.ownerType ?? ""}
          disabled={disabled}
          onChange={(event) =>
            handleOwnerTypeChange(event.target.value as OwnerType | "")
          }
          className="mt-1 w-full rounded-md border border-gray-300 px-3 py-2 text-sm disabled:cursor-not-allowed disabled:bg-gray-100"
        >
          {props.includeAny && <option value="">Any</option>}
          {allowedOwnerTypes.map((ownerType) => (
            <option key={ownerType} value={ownerType}>
              {ownerTypeLabel(ownerType)}
            </option>
          ))}
        </select>
      </div>

      {value?.ownerType === OwnerType.SYSTEM && (
        <div className="flex items-end pb-2 text-xs text-gray-500">
          System-owned namespace. No owner reference is required.
        </div>
      )}

      {value?.ownerType === OwnerType.IDENTITY && (
        <div className="flex items-end pb-2 text-xs text-gray-500">
          Scoped to your own identity ({user?.login ?? "current user"}).
          Cross-identity cache ownership is not supported yet.
        </div>
      )}

      {value?.ownerType === OwnerType.PACK && (
        <div>
          <label className="block text-xs font-medium uppercase tracking-wide text-gray-500">
            {ownerRefLabelText}
          </label>
          <SearchableSelect
            ariaLabel="Pack reference"
            options={packOptions}
            value={value.ownerRef}
            disabled={disabled || packsLoading}
            placeholder="Select a pack…"
            onChange={(next) =>
              props.onChange({ ...value, ownerRef: String(next) })
            }
            className="mt-1"
          />
        </div>
      )}

      {value?.ownerType === OwnerType.ACTION && (
        <div>
          <label className="block text-xs font-medium uppercase tracking-wide text-gray-500">
            {ownerRefLabelText}
          </label>
          <SearchableSelect
            ariaLabel="Action reference"
            options={actionOptions}
            value={value.ownerRef}
            disabled={disabled || actionsLoading}
            placeholder="Select an action…"
            onChange={(next) =>
              props.onChange({ ...value, ownerRef: String(next) })
            }
            className="mt-1"
          />
        </div>
      )}

      {value?.ownerType === OwnerType.SENSOR && (
        <div>
          <label className="block text-xs font-medium uppercase tracking-wide text-gray-500">
            {ownerRefLabelText}
          </label>
          <SearchableSelect
            ariaLabel="Sensor reference"
            options={sensorOptions}
            value={value.ownerRef}
            disabled={disabled || sensorsLoading}
            placeholder="Select a sensor…"
            onChange={(next) =>
              props.onChange({ ...value, ownerRef: String(next) })
            }
            className="mt-1"
          />
        </div>
      )}
    </div>
  );
}
