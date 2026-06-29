interface OnOffSwitchProps {
  checked: boolean;
  disabled?: boolean;
  onChange?: (checked: boolean) => void;
  ariaLabel?: string;
  stopPropagation?: boolean;
}

export default function OnOffSwitch({
  checked,
  disabled = false,
  onChange,
  ariaLabel,
  stopPropagation = false,
}: OnOffSwitchProps) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={ariaLabel}
      disabled={disabled}
      onClick={(event) => {
        if (stopPropagation) {
          event.stopPropagation();
        }
        if (!disabled) {
          onChange?.(!checked);
        }
      }}
      onKeyDown={(event) => {
        if (stopPropagation) {
          event.stopPropagation();
        }
      }}
      className={`relative inline-flex h-7 w-14 items-center rounded-full border px-1 text-[10px] font-semibold transition-colors focus:outline-none focus:ring-2 focus:ring-blue-500 focus:ring-offset-2 ${
        checked
          ? "border-blue-600 bg-blue-600 text-white"
          : "border-gray-300 bg-gray-200 text-gray-600"
      } ${disabled ? "cursor-not-allowed opacity-60" : "cursor-pointer"}`}
    >
      <span
        className={`inline-flex h-5 w-7 items-center justify-center rounded-full bg-white text-[10px] font-bold shadow transition-transform ${
          checked
            ? "translate-x-5 text-blue-700"
            : "translate-x-0 text-gray-500"
        }`}
      >
        {checked ? "ON" : "OFF"}
      </span>
    </button>
  );
}
