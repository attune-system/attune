import { useMemo } from "react";
import { useSearchParams } from "react-router-dom";
import { usePacks } from "@/hooks/usePacks";
import SearchableSelect from "@/components/common/SearchableSelect";

interface PackFilterProps {
  className?: string;
  label?: string;
}

export default function PackFilter({ className, label }: PackFilterProps) {
  const [searchParams, setSearchParams] = useSearchParams();
  const selectedPack = searchParams.get("pack")?.trim() || "";
  const { data: packsData, isLoading } = usePacks({ pageSize: 1000 });
  const options = useMemo(() => {
    const packRefs = (packsData?.items ?? []).map((pack) => pack.ref);

    if (selectedPack && !packRefs.includes(selectedPack)) {
      packRefs.push(selectedPack);
    }

    return packRefs
      .sort((left, right) => left.localeCompare(right))
      .map((ref) => ({ value: ref, label: ref }));
  }, [packsData?.items, selectedPack]);

  const updatePack = (value: string | number) => {
    const packRef = String(value).trim();
    const next = new URLSearchParams(searchParams);

    if (packRef) {
      next.set("pack", packRef);
    } else {
      next.delete("pack");
    }

    setSearchParams(next);
  };

  return (
    <div className={className}>
      {label && (
        <label className="mb-1 block text-sm font-medium text-gray-700">
          {label}
        </label>
      )}
      <SearchableSelect
        id="pack-filter"
        ariaLabel="Filter by pack"
        options={options}
        value={selectedPack}
        onChange={updatePack}
        placeholder={isLoading ? "Loading packs..." : "All packs"}
        disabled={isLoading && !selectedPack}
      />
    </div>
  );
}
