import SearchableSelect from "@/components/common/SearchableSelect";
import {
  MAX_ALLOWED_ACTION_REFS,
  validateAllowedActionRefs,
  type PaletteAction,
  type WorkflowTask,
} from "@/types/workflow";

interface TaskActionSelectorProps {
  task: Pick<WorkflowTask, "action" | "allowed_action_refs">;
  path: string;
  availableActions: PaletteAction[];
  onChange: (updates: Partial<WorkflowTask>) => void;
}

export default function TaskActionSelector({
  task,
  path,
  availableActions,
  onChange,
}: TaskActionSelectorProps) {
  const targets = task.allowed_action_refs ?? [];
  const dynamic = task.action.includes("{{");
  const errors = validateAllowedActionRefs(task, path);
  const setTarget = (index: number, reference: string) =>
    onChange({
      allowed_action_refs: targets.map((target, position) =>
        position === index ? reference : target,
      ),
    });

  return (
    <div className="space-y-3">
      <label className="block text-xs font-medium text-gray-700">
        Action selector
        <input
          aria-label="Action selector"
          value={task.action}
          onChange={(event) => onChange({ action: event.target.value })}
          className="mt-1 w-full px-2.5 py-1.5 border border-gray-300 rounded text-xs font-mono"
          placeholder="example.action or {{ parameters.target }}"
        />
      </label>
      <SearchableSelect
        ariaLabel="Referenceable action suggestions"
        value=""
        options={availableActions.map((action) => ({
          value: action.ref,
          label: `${action.ref} (${action.label})`,
        }))}
        onChange={(reference) => {
          if (reference) onChange({ action: String(reference) });
        }}
        placeholder="Choose a literal action"
      />
      <fieldset className="space-y-2">
        <legend className="text-xs font-medium text-gray-700">
          Allowed action targets
        </legend>
        <p className="text-xs text-gray-500">
          A dynamic selector requires 1 to {MAX_ALLOWED_ACTION_REFS} literal,
          fully qualified targets. No wildcards or templates. A literal selector
          must belong to its declared ceiling.
        </p>
        {targets.map((target, index) => (
          <div className="flex gap-1" key={index}>
            <input
              aria-label={`Allowed action target ${index + 1}`}
              value={target}
              onChange={(event) => setTarget(index, event.target.value)}
              onBlur={() => setTarget(index, target.trim())}
              className="min-w-0 flex-1 px-2 py-1 border border-gray-300 rounded text-xs font-mono"
              placeholder="pack.action"
            />
            <button
              type="button"
              aria-label={`Remove action target ${index + 1}`}
              onClick={() =>
                onChange({
                  allowed_action_refs: targets.filter(
                    (_, position) => position !== index,
                  ),
                })
              }
            >
              Remove
            </button>
          </div>
        ))}
        <button
          type="button"
          onClick={() => onChange({ allowed_action_refs: [...targets, ""] })}
          disabled={targets.length >= MAX_ALLOWED_ACTION_REFS}
          className="text-xs text-blue-600 disabled:text-gray-400"
        >
          Add action target
        </button>
        <SearchableSelect
          ariaLabel="Referenceable target suggestions"
          value=""
          disabled={targets.length >= MAX_ALLOWED_ACTION_REFS}
          options={availableActions.map((action) => ({
            value: action.ref,
            label: action.ref,
          }))}
          onChange={(reference) => {
            if (reference && !targets.includes(String(reference)))
              onChange({
                allowed_action_refs: [...targets, String(reference)],
              });
          }}
          placeholder="Add a discovered target"
        />
        {!dynamic && task.allowed_action_refs !== undefined && (
          <button
            type="button"
            className="text-xs text-gray-600"
            onClick={() => onChange({ allowed_action_refs: undefined })}
          >
            Remove optional target ceiling
          </button>
        )}
      </fieldset>
      {errors.length > 0 && (
        <ul role="alert" className="text-xs text-red-600">
          {errors.map((error) => (
            <li key={error}>{error}</li>
          ))}
        </ul>
      )}
      <p className="text-xs text-gray-500">
        Discovery comes from the server, not permission-set membership. You can
        enter refs that are not listed. The server rechecks ownership, foreign
        private/restricted references and approvals on save. Choosing a target
        does not grant runtime capabilities.
      </p>
    </div>
  );
}
