import { useState, useMemo } from "react";
import {
  Search,
  X,
  ChevronDown,
  ChevronRight,
  GripVertical,
} from "lucide-react";
import type { PaletteAction } from "@/types/workflow";
import PackIcon from "@/components/common/PackIcon";

interface ActionPaletteProps {
  actions: PaletteAction[];
  isLoading: boolean;
  onAddTask: (action: PaletteAction) => void;
}

export default function ActionPalette({
  actions,
  isLoading,
  onAddTask,
}: ActionPaletteProps) {
  const [searchQuery, setSearchQuery] = useState("");
  const [collapsedPacks, setCollapsedPacks] = useState<Set<string>>(new Set());

  const filteredActions = useMemo(() => {
    if (!searchQuery.trim()) return actions;
    const query = searchQuery.toLowerCase();
    return actions.filter(
      (action) =>
        action.label?.toLowerCase().includes(query) ||
        action.ref?.toLowerCase().includes(query) ||
        action.description?.toLowerCase().includes(query) ||
        action.pack_ref?.toLowerCase().includes(query),
    );
  }, [actions, searchQuery]);

  const actionsByPack = useMemo(() => {
    const grouped = new Map<string, PaletteAction[]>();
    filteredActions.forEach((action) => {
      const packRef = action.pack_ref;
      if (!grouped.has(packRef)) {
        grouped.set(packRef, []);
      }
      grouped.get(packRef)!.push(action);
    });
    return new Map(
      [...grouped.entries()].sort((a, b) => a[0].localeCompare(b[0])),
    );
  }, [filteredActions]);

  const togglePack = (packRef: string) => {
    setCollapsedPacks((prev) => {
      const next = new Set(prev);
      if (next.has(packRef)) {
        next.delete(packRef);
      } else {
        next.add(packRef);
      }
      return next;
    });
  };

  return (
    <div className="flex flex-col h-full overflow-hidden">
      <div className="p-3 border-b border-gray-200 bg-white flex-shrink-0">
        <h3 className="text-sm font-semibold text-gray-700 uppercase tracking-wider mb-2">
          Action Palette
        </h3>
        <div className="relative">
          <div className="absolute inset-y-0 left-0 pl-2 flex items-center pointer-events-none">
            <Search className="h-3.5 w-3.5 text-gray-400" />
          </div>
          <input
            type="text"
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            placeholder="Search actions..."
            className="block w-full pl-8 pr-8 py-1.5 border border-gray-300 rounded text-xs focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
          />
          {searchQuery && (
            <button
              onClick={() => setSearchQuery("")}
              className="absolute inset-y-0 right-0 pr-2 flex items-center"
            >
              <X className="h-3.5 w-3.5 text-gray-400 hover:text-gray-600" />
            </button>
          )}
        </div>
      </div>

      <div className="flex-1 overflow-y-auto p-2">
        {isLoading ? (
          <div className="flex items-center justify-center py-8">
            <div className="animate-spin rounded-full h-6 w-6 border-b-2 border-blue-600" />
          </div>
        ) : actions.length === 0 ? (
          <div className="text-center py-8 text-xs text-gray-500">
            No actions available
          </div>
        ) : filteredActions.length === 0 ? (
          <div className="text-center py-8">
            <p className="text-xs text-gray-500">
              No actions match your search
            </p>
            <button
              onClick={() => setSearchQuery("")}
              className="mt-1 text-xs text-blue-600 hover:text-blue-800"
            >
              Clear search
            </button>
          </div>
        ) : (
          <div className="space-y-1">
            {Array.from(actionsByPack.entries()).map(
              ([packRef, packActions]) => {
                const isCollapsed = collapsedPacks.has(packRef);
                return (
                  <div key={packRef} className="rounded overflow-hidden">
                    <button
                      onClick={() => togglePack(packRef)}
                      className="w-full px-2 py-1.5 flex items-center justify-between hover:bg-gray-100 transition-colors text-left"
                    >
                      <div className="flex items-center gap-1.5">
                        {isCollapsed ? (
                          <ChevronRight className="w-3 h-3 text-gray-500 flex-shrink-0" />
                        ) : (
                          <ChevronDown className="w-3 h-3 text-gray-500 flex-shrink-0" />
                        )}
                        <PackIcon packRef={packRef} size="xs" />
                        <span className="font-semibold text-xs text-gray-800 truncate">
                          {packRef}
                        </span>
                      </div>
                      <span className="text-[10px] text-gray-500 bg-gray-200 px-1.5 py-0.5 rounded flex-shrink-0">
                        {packActions.length}
                      </span>
                    </button>

                    {!isCollapsed && (
                      <div className="pl-1 pb-1">
                        {packActions.map((action) => (
                          <button
                            key={action.id}
                            onClick={() => onAddTask(action)}
                            className="w-full text-left px-2 py-1.5 rounded hover:bg-blue-50 hover:border-blue-200 border border-transparent transition-colors group cursor-pointer"
                            title={`Click to add "${action.label}" as a task`}
                          >
                            <div className="flex items-start gap-1.5">
                              <GripVertical className="w-3 h-3 text-gray-300 group-hover:text-blue-400 mt-0.5 flex-shrink-0" />
                              <PackIcon
                                packRef={action.pack_ref}
                                size="xs"
                                className="mt-0.5"
                              />
                              <div className="min-w-0 flex-1">
                                <div className="font-medium text-xs text-gray-900 truncate">
                                  {action.label}
                                </div>
                                <div className="font-mono text-[10px] text-gray-500 truncate">
                                  {action.ref}
                                </div>
                                {action.description && (
                                  <div className="text-[10px] text-gray-400 truncate mt-0.5">
                                    {action.description}
                                  </div>
                                )}
                              </div>
                            </div>
                          </button>
                        ))}
                      </div>
                    )}
                  </div>
                );
              },
            )}
          </div>
        )}
      </div>
    </div>
  );
}
