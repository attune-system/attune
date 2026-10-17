export default function PermissionScopeControl({
  value,
  onChange,
}: {
  value: "pack" | "external";
  onChange: (value: "pack" | "external") => void;
}) {
  return (
    <label className="block text-sm text-gray-700">
      Default authority scope
      <select
        aria-label="Default authority scope"
        value={value}
        onChange={(event) => {
          const selected = event.target.value;
          if (selected === "pack" || selected === "external")
            onChange(selected);
        }}
        className="mt-1 w-full rounded-md border border-gray-300 px-3 py-2 text-sm"
      >
        <option value="pack">
          Local: actual structural ownership by this pack
        </option>
        <option value="external">
          External: absolute scope, with approval for managed use
        </option>
      </select>
      <span className="mt-1 block text-xs text-gray-500">
        Individual grants may override this default. Local scope never
        authorizes a foreign resource merely because its reference looks local.
      </span>
    </label>
  );
}
