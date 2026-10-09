import { CacheRefreshConcurrency } from "@/api";

export default function CacheRefreshConcurrencySelect({
  value,
  onChange,
}: {
  value: CacheRefreshConcurrency;
  onChange: (value: CacheRefreshConcurrency) => void;
}) {
  return (
    <div>
      <label
        htmlFor="refresh-concurrency"
        className="block text-sm font-medium text-gray-700"
      >
        Refresh concurrency
      </label>
      <select
        id="refresh-concurrency"
        value={value}
        onChange={(event) => {
          const selected = Object.values(CacheRefreshConcurrency).find(
            (mode) => mode === event.target.value,
          );
          if (selected) onChange(selected);
        }}
        className="mt-1 w-full rounded-md border border-gray-300 px-3 py-2 text-sm"
      >
        <option value={CacheRefreshConcurrency.PARALLEL}>Parallel</option>
        <option value={CacheRefreshConcurrency.REUSE}>Reuse</option>
        <option value={CacheRefreshConcurrency.CONFLICT}>Conflict</option>
      </select>
      <p className="mt-1 text-xs text-gray-500">
        Parallel allows separate refreshes within the staging quotas. Reuse
        returns the oldest staging or ready generation. Conflict rejects another
        refresh while one is staging or ready.
      </p>
    </div>
  );
}
