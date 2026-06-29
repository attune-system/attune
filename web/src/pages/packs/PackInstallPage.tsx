import { useEffect, useState } from "react";
import { useNavigate, Link } from "react-router-dom";
import { useInstallPack } from "@/hooks/usePackTests";
import {
  useCreatePackIndex,
  useDeletePackIndex,
  useIndexedPacks,
  usePackIndices,
  useUpdatePackIndex,
} from "@/hooks/usePacks";
import {
  AlertCircle,
  CheckCircle,
  Loader2,
  GitBranch,
  Package,
  Info,
  Settings,
  X,
  GripVertical,
} from "lucide-react";

type SourceType = "git" | "archive" | "registry";
type IndexedPackContentKey =
  "actions" | "sensors" | "triggers" | "rules" | "workflows";

interface PackRegistryIndex {
  id: number;
  name?: string | null;
  url: string;
  position: number;
  enabled: boolean;
}

interface IndexedPackEntry {
  ref: string;
  label?: string | null;
  description?: string | null;
  use_case?: string | null;
  version?: string | null;
  contents?: Partial<Record<IndexedPackContentKey, unknown[]>>;
}

interface IndexedPackResult {
  pack: IndexedPackEntry;
  registry: {
    name?: string | null;
    url: string;
  };
}

export default function PackInstallPage() {
  const navigate = useNavigate();
  const installPack = useInstallPack();
  const packIndices = usePackIndices();
  const createPackIndex = useCreatePackIndex();
  const updatePackIndex = useUpdatePackIndex();
  const deletePackIndex = useDeletePackIndex();

  const [sourceType, setSourceType] = useState<SourceType>("git");
  const [isIndexModalOpen, setIsIndexModalOpen] = useState(false);
  const [indexForm, setIndexForm] = useState({
    name: "",
    url: "",
  });
  const [draggedIndexId, setDraggedIndexId] = useState<number | null>(null);
  const [packQuery, setPackQuery] = useState("");
  const indexedPacks = useIndexedPacks(packQuery);
  const [formData, setFormData] = useState({
    source: "",
    refSpec: "",
    skipTests: false,
    skipDeps: false,
  });
  const configuredIndices = (packIndices.data?.data ??
    []) as PackRegistryIndex[];
  const indexedPackResults = (indexedPacks.data?.data ??
    []) as IndexedPackResult[];

  const [error, setError] = useState<string | null>(null);
  const [success, setSuccess] = useState<string | null>(null);
  const [installErrorToast, setInstallErrorToast] = useState<string | null>(
    null,
  );

  useEffect(() => {
    if (!installErrorToast) return;
    const timeout = window.setTimeout(() => {
      setInstallErrorToast(null);
    }, 7000);
    return () => window.clearTimeout(timeout);
  }, [installErrorToast]);

  const getErrorMessage = (err: unknown, fallback: string): string => {
    const maybeApiError = err as {
      body?: { error?: string; message?: string };
    };
    const maybeAxios = err as {
      response?: { data?: { error?: string; message?: string } };
    };
    return (
      maybeApiError.body?.error ||
      maybeApiError.body?.message ||
      maybeAxios.response?.data?.error ||
      maybeAxios.response?.data?.message ||
      (err instanceof Error ? err.message : fallback)
    );
  };

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    setError(null);
    setSuccess(null);

    if (!formData.source) {
      setError("Pack source is required");
      return;
    }

    try {
      const result = await installPack.mutateAsync({
        source: formData.source,
        refSpec: formData.refSpec || undefined,
        skipTests: formData.skipTests,
        skipDeps: formData.skipDeps,
      });

      const packRef = result.data.pack.ref;
      setSuccess(
        `Pack '${result.data.pack.label}' (${result.data.pack.version}) installed successfully! ${
          result.data.tests_skipped
            ? "Tests were skipped."
            : result.data.test_result
              ? `Tests ${result.data.test_result.status}: ${result.data.test_result.passed}/${result.data.test_result.totalTests} passed.`
              : ""
        }`,
      );

      // Redirect to pack details after 2 seconds
      setTimeout(() => {
        navigate(`/packs/${packRef}`);
      }, 2000);
    } catch (err) {
      const message = getErrorMessage(err, "Failed to install pack");
      setError(message);
      setInstallErrorToast(message);
    }
  };

  const handleAddIndex = async (e: React.FormEvent) => {
    e.preventDefault();
    setError(null);
    try {
      await createPackIndex.mutateAsync({
        name: indexForm.name || undefined,
        url: indexForm.url,
        enabled: true,
        headers: {},
      });
      setIndexForm({ name: "", url: "" });
    } catch (err) {
      setError((err as Error).message || "Failed to add pack index");
    }
  };

  const handleDropIndex = async (targetId: number) => {
    if (draggedIndexId === null || draggedIndexId === targetId) {
      setDraggedIndexId(null);
      return;
    }

    const indices = [...configuredIndices];
    const fromIndex = indices.findIndex((index) => index.id === draggedIndexId);
    const toIndex = indices.findIndex((index) => index.id === targetId);
    if (fromIndex === -1 || toIndex === -1) {
      setDraggedIndexId(null);
      return;
    }

    const [moved] = indices.splice(fromIndex, 1);
    indices.splice(toIndex, 0, moved);
    setDraggedIndexId(null);
    setError(null);

    try {
      await Promise.all(
        indices.map((index, position) =>
          index.position === position
            ? Promise.resolve()
            : updatePackIndex.mutateAsync({
                id: index.id,
                data: { position },
              }),
        ),
      );
    } catch (err) {
      setError((err as Error).message || "Failed to reorder pack indices");
    }
  };

  const selectIndexedPack = (packRef: string) => {
    setSourceType("registry");
    setFormData((prev) => ({ ...prev, source: packRef, refSpec: "" }));
  };

  const handleChange = (
    e: React.ChangeEvent<HTMLInputElement | HTMLSelectElement>,
  ) => {
    const target = e.target as HTMLInputElement;
    const { name, type, checked, value } = target;
    setFormData((prev) => ({
      ...prev,
      [name]: type === "checkbox" ? checked : value,
    }));
  };

  const getSourcePlaceholder = () => {
    switch (sourceType) {
      case "git":
        return "https://github.com/example/pack-slack.git";
      case "archive":
        return "https://example.com/packs/pack-slack-1.0.0.tar.gz";
      case "registry":
        return "slack";
      default:
        return "";
    }
  };

  const getSourceLabel = () => {
    switch (sourceType) {
      case "git":
        return "Git Repository URL";
      case "archive":
        return "Archive URL";
      case "registry":
        return "Pack Reference";
      default:
        return "Source";
    }
  };

  const showRefSpec = sourceType === "git" || sourceType === "registry";

  return (
    <div className="p-6 max-w-3xl mx-auto">
      <div className="mb-6">
        <Link
          to="/packs"
          className="text-blue-600 hover:text-blue-800 mb-2 inline-block"
        >
          ← Back to Packs
        </Link>
        <h1 className="text-3xl font-bold">Install Pack</h1>
        <p className="mt-2 text-gray-600">
          Install a pack from git, archive URL, or pack registry
        </p>
        <button
          type="button"
          onClick={() => setIsIndexModalOpen(true)}
          className="mt-4 inline-flex items-center gap-2 px-3 py-2 border border-gray-300 text-gray-700 rounded-lg hover:bg-gray-50 transition-colors text-sm font-medium"
        >
          <Settings className="w-4 h-4" />
          Manage Pack Indices
        </button>
      </div>

      {/* Info Box */}
      <div className="mb-6 bg-blue-50 border border-blue-200 rounded-lg p-5">
        <div className="flex items-start gap-3">
          <Package className="w-5 h-5 text-blue-600 flex-shrink-0 mt-0.5" />
          <div>
            <h3 className="text-sm font-semibold text-blue-900 mb-2">
              Remote Pack Installation
            </h3>
            <p className="text-sm text-blue-800 mb-2">
              This option installs a pack from a remote source:
            </p>
            <ul className="text-sm text-blue-800 space-y-1 list-disc list-inside ml-2">
              <li>
                <strong>Git Repository</strong> - Clone from GitHub, GitLab, or
                any git server
              </li>
              <li>
                <strong>Archive URL</strong> - Download from .zip or .tar.gz URL
              </li>
              <li>
                <strong>Pack Registry</strong> - Browse configured indices and
                install the first matching pack ref by index order
              </li>
            </ul>
            <div className="mt-3 pt-3 border-t border-blue-300">
              <p className="text-xs text-blue-700">
                <strong>Local development?</strong> Use the CLI or API directly
                for server-side filesystem paths; browsers cannot access server
                pack directories.
              </p>
            </div>
          </div>
        </div>
      </div>

      {error && (
        <div className="mb-6 bg-red-50 border border-red-200 rounded-lg p-4 flex items-start gap-3">
          <AlertCircle className="w-5 h-5 text-red-600 flex-shrink-0 mt-0.5" />
          <div className="flex-1">
            <h3 className="text-sm font-semibold text-red-800">Error</h3>
            <p className="text-sm text-red-700 mt-1 whitespace-pre-line">
              {error}
            </p>
          </div>
        </div>
      )}

      {success && (
        <div className="mb-6 bg-green-50 border border-green-200 rounded-lg p-4 flex items-start gap-3">
          <CheckCircle className="w-5 h-5 text-green-600 flex-shrink-0 mt-0.5" />
          <div className="flex-1">
            <h3 className="text-sm font-semibold text-green-800">Success</h3>
            <p className="text-sm text-green-700 mt-1">{success}</p>
            <p className="text-xs text-green-600 mt-2">
              Redirecting to pack details...
            </p>
          </div>
        </div>
      )}

      {installErrorToast && (
        <div className="fixed top-20 right-4 z-50 max-w-xl">
          <div className="flex items-start gap-2.5 rounded-lg border border-red-300 bg-red-50 px-4 py-3 shadow-lg">
            <AlertCircle className="mt-0.5 h-4 w-4 flex-shrink-0 text-red-500" />
            <div className="flex-1">
              <p className="text-sm font-semibold text-red-900">
                Pack installation failed
              </p>
              <p className="mt-0.5 text-xs whitespace-pre-line text-red-800">
                {installErrorToast}
              </p>
            </div>
            <button
              type="button"
              onClick={() => setInstallErrorToast(null)}
              className="rounded p-0.5 text-red-400 hover:bg-black/5 hover:text-red-600"
              aria-label="Dismiss install error"
            >
              <X className="h-3.5 w-3.5" />
            </button>
          </div>
        </div>
      )}

      <div className="bg-white shadow rounded-lg">
        <form onSubmit={handleSubmit} className="p-6 space-y-6">
          {/* Source Type Selection */}
          <div>
            <label className="block text-sm font-medium text-gray-700 mb-2">
              Installation Source Type <span className="text-red-500">*</span>
            </label>
            <div className="grid grid-cols-3 gap-3">
              <button
                type="button"
                onClick={() => setSourceType("git")}
                className={`px-4 py-3 border rounded-lg text-sm font-medium transition-colors ${
                  sourceType === "git"
                    ? "border-blue-500 bg-blue-50 text-blue-700"
                    : "border-gray-300 bg-white text-gray-700 hover:bg-gray-50"
                }`}
              >
                <GitBranch className="w-4 h-4 mx-auto mb-1" />
                Git Repository
              </button>
              <button
                type="button"
                onClick={() => setSourceType("archive")}
                className={`px-4 py-3 border rounded-lg text-sm font-medium transition-colors ${
                  sourceType === "archive"
                    ? "border-blue-500 bg-blue-50 text-blue-700"
                    : "border-gray-300 bg-white text-gray-700 hover:bg-gray-50"
                }`}
              >
                <Package className="w-4 h-4 mx-auto mb-1" />
                Archive URL
              </button>
              <button
                type="button"
                onClick={() => setSourceType("registry")}
                className={`px-4 py-3 border rounded-lg text-sm font-medium transition-colors ${
                  sourceType === "registry"
                    ? "border-blue-500 bg-blue-50 text-blue-700"
                    : "border-gray-300 bg-white text-gray-700 hover:bg-gray-50"
                }`}
              >
                <Package className="w-4 h-4 mx-auto mb-1" />
                Registry
              </button>
            </div>
          </div>

          {/* Source Input */}
          {sourceType === "registry" ? (
            <div>
              <div className="flex items-center justify-between gap-3 mb-2">
                <label
                  htmlFor="packQuery"
                  className="block text-sm font-medium text-gray-700"
                >
                  Search Indexed Packs <span className="text-red-500">*</span>
                </label>
                <button
                  type="button"
                  onClick={() => setIsIndexModalOpen(true)}
                  className="text-sm text-blue-600 hover:text-blue-800"
                >
                  Manage indices
                </button>
              </div>
              <input
                type="search"
                id="packQuery"
                value={packQuery}
                onChange={(e) => setPackQuery(e.target.value)}
                placeholder="Search by ref, label, keyword, or use case"
                className="w-full px-3 py-2 border border-gray-300 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent"
              />
              {formData.source && (
                <div className="mt-3 rounded-lg border border-blue-200 bg-blue-50 px-3 py-2 text-sm text-blue-800">
                  Selected pack ref:{" "}
                  <span className="font-medium">{formData.source}</span>
                </div>
              )}
              <div className="mt-3 space-y-3 max-h-96 overflow-auto">
                {indexedPackResults.map((indexed) => {
                  const pack = indexed.pack;
                  const contents = pack.contents || {};
                  const contentKeys: IndexedPackContentKey[] = [
                    "actions",
                    "sensors",
                    "triggers",
                    "rules",
                    "workflows",
                  ];
                  const counts = contentKeys
                    .map((key) => `${key}: ${(contents[key] || []).length}`)
                    .join(" · ");
                  return (
                    <button
                      key={`${indexed.registry.url}:${pack.ref}`}
                      type="button"
                      onClick={() => selectIndexedPack(pack.ref)}
                      className={`w-full text-left border rounded-lg p-3 hover:bg-blue-50 ${
                        formData.source === pack.ref
                          ? "border-blue-500 bg-blue-50"
                          : "border-gray-200"
                      }`}
                    >
                      <div className="flex items-center justify-between gap-3">
                        <div className="font-medium">
                          {pack.label || pack.ref}
                        </div>
                        <div className="text-xs text-gray-500">
                          {pack.version}
                        </div>
                      </div>
                      <div className="text-sm text-gray-600 mt-1">
                        {pack.use_case || pack.description}
                      </div>
                      <div className="text-xs text-gray-500 mt-2">{counts}</div>
                      <div className="text-xs text-gray-400 mt-1">
                        {indexed.registry.name || indexed.registry.url}
                      </div>
                    </button>
                  );
                })}
                {indexedPacks.isLoading && (
                  <p className="text-sm text-gray-500">
                    Loading indexed packs...
                  </p>
                )}
                {!indexedPacks.isLoading && indexedPackResults.length === 0 && (
                  <p className="text-sm text-gray-500">
                    No indexed packs found. Add an index or adjust your search.
                  </p>
                )}
              </div>
            </div>
          ) : (
            <div>
              <label
                htmlFor="source"
                className="block text-sm font-medium text-gray-700 mb-2"
              >
                {getSourceLabel()} <span className="text-red-500">*</span>
              </label>
              <input
                type="text"
                id="source"
                name="source"
                value={formData.source}
                onChange={handleChange}
                placeholder={getSourcePlaceholder()}
                className="w-full px-3 py-2 border border-gray-300 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent"
                required
              />
              {sourceType === "git" && (
                <p className="mt-2 text-sm text-gray-500">
                  Supports HTTPS and SSH URLs. Examples:
                  <br />
                  • https://github.com/username/pack-name.git
                  <br />• git@github.com:username/pack-name.git
                </p>
              )}
              {sourceType === "archive" && (
                <p className="mt-2 text-sm text-gray-500">
                  Direct URL to .zip or .tar.gz archive containing pack files
                </p>
              )}
            </div>
          )}

          {/* Git Reference (for git and registry sources) */}
          {showRefSpec && (
            <div>
              <label
                htmlFor="refSpec"
                className="block text-sm font-medium text-gray-700 mb-2"
              >
                {sourceType === "git" ? "Git Reference" : "Version"}
                <span className="text-gray-500 text-xs ml-2">(Optional)</span>
              </label>
              <input
                type="text"
                id="refSpec"
                name="refSpec"
                value={formData.refSpec}
                onChange={handleChange}
                placeholder={
                  sourceType === "git"
                    ? "main, v1.0.0, commit-hash, etc."
                    : "1.0.0, latest, etc."
                }
                className="w-full px-3 py-2 border border-gray-300 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent"
              />
              {sourceType === "git" && (
                <p className="mt-2 text-sm text-gray-500">
                  Branch name, tag, or commit hash. Defaults to default branch
                  if not specified.
                </p>
              )}
              {sourceType === "registry" && (
                <p className="mt-2 text-sm text-gray-500">
                  Specific version or "latest". Defaults to latest if not
                  specified.
                </p>
              )}
            </div>
          )}

          {/* Installation Options */}
          <div className="border-t border-gray-200 pt-6">
            <h3 className="text-lg font-semibold text-gray-900 mb-4">
              Installation Options
            </h3>

            <div className="space-y-4">
              {/* Skip Dependencies */}
              <div className="flex items-start">
                <div className="flex items-center h-5">
                  <input
                    type="checkbox"
                    id="skipDeps"
                    name="skipDeps"
                    checked={formData.skipDeps}
                    onChange={handleChange}
                    className="w-4 h-4 text-blue-600 border-gray-300 rounded focus:ring-blue-500"
                  />
                </div>
                <div className="ml-3">
                  <label
                    htmlFor="skipDeps"
                    className="text-sm font-medium text-gray-700"
                  >
                    Skip Dependency Validation
                  </label>
                  <p className="text-sm text-gray-500">
                    Skip checking for required runtime dependencies and pack
                    dependencies. Use with caution.
                  </p>
                </div>
              </div>

              {/* Skip Tests */}
              <div className="flex items-start">
                <div className="flex items-center h-5">
                  <input
                    type="checkbox"
                    id="skipTests"
                    name="skipTests"
                    checked={formData.skipTests}
                    onChange={handleChange}
                    className="w-4 h-4 text-blue-600 border-gray-300 rounded focus:ring-blue-500"
                  />
                </div>
                <div className="ml-3">
                  <label
                    htmlFor="skipTests"
                    className="text-sm font-medium text-gray-700"
                  >
                    Skip Tests
                  </label>
                  <p className="text-sm text-gray-500">
                    Skip running pack tests during installation. Useful when
                    tests are not available or trusted.
                  </p>
                </div>
              </div>
            </div>
          </div>

          {/* Info Box */}
          <div className="bg-amber-50 border border-amber-200 rounded-lg p-4">
            <div className="flex items-start gap-3">
              <Info className="w-5 h-5 text-amber-600 flex-shrink-0 mt-0.5" />
              <div>
                <h4 className="text-sm font-semibold text-amber-900 mb-2">
                  Installation Process
                </h4>
                <ul className="text-sm text-amber-800 space-y-1 list-disc list-inside">
                  <li>Pack is downloaded from the specified source</li>
                  {!formData.skipDeps && (
                    <li className="font-medium">
                      Dependencies are validated (runtime & pack dependencies)
                    </li>
                  )}
                  <li>Pack metadata is registered in the database</li>
                  <li>
                    Pack files are copied to permanent storage (
                    {sourceType === "git" && "cloned from git"}
                    {sourceType === "archive" && "extracted from archive"}
                    {sourceType === "registry" && "downloaded from registry"})
                  </li>
                  <li>Workflows are automatically synced</li>
                  {!formData.skipTests && (
                    <li className="font-medium">
                      Tests are executed and must pass (unless forced)
                    </li>
                  )}
                  {formData.skipDeps && (
                    <li className="text-amber-600 font-medium">
                      ⚠️ Dependency validation will be skipped
                    </li>
                  )}
                  {formData.skipTests && (
                    <li className="text-amber-600 font-medium">
                      ⚠️ Tests will be skipped
                    </li>
                  )}
                </ul>
              </div>
            </div>
          </div>

          {/* Actions */}
          <div className="flex items-center gap-3 pt-6 border-t border-gray-200">
            <button
              type="submit"
              disabled={installPack.isPending || !!success}
              className="px-6 py-2 bg-blue-600 text-white rounded-lg hover:bg-blue-700 disabled:opacity-50 disabled:cursor-not-allowed transition-colors font-medium flex items-center gap-2"
            >
              {installPack.isPending ? (
                <>
                  <Loader2 className="w-4 h-4 animate-spin" />
                  Installing...
                </>
              ) : (
                <>
                  <Package className="w-4 h-4" />
                  Install Pack
                </>
              )}
            </button>
            <Link
              to="/packs"
              className="px-6 py-2 border border-gray-300 text-gray-700 rounded-lg hover:bg-gray-50 transition-colors font-medium"
            >
              Cancel
            </Link>
          </div>
        </form>
      </div>

      {/* Help Section */}
      <div className="mt-6 bg-gray-50 rounded-lg p-6">
        <h3 className="text-lg font-semibold text-gray-900 mb-3">
          Installation Requirements
        </h3>
        <div className="space-y-3 text-sm text-gray-600">
          <div>
            <strong className="text-gray-900">Pack Structure:</strong> The
            source must contain a valid pack.yaml file with pack metadata,
            either at the root or in a pack/ subdirectory
          </div>
          <div>
            <strong className="text-gray-900">Git Installation:</strong>{" "}
            Requires git to be installed on the Attune server. Supports HTTPS
            and SSH URLs.
          </div>
          <div>
            <strong className="text-gray-900">Dependencies:</strong> Pack
            dependencies (runtime and other packs) are validated before
            installation unless skipped
          </div>
          <div>
            <strong className="text-gray-900">Testing:</strong> If tests are
            configured in pack.yaml, they will run automatically unless skipped
          </div>
          <div>
            <strong className="text-gray-900">Force Mode:</strong> Use this to
            replace existing packs, bypass dependency checks, or proceed despite
            test failures
          </div>
        </div>
      </div>

      {isIndexModalOpen && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4">
          <div className="w-full max-w-2xl max-h-[90vh] overflow-auto rounded-lg bg-white shadow-xl">
            <div className="flex items-center justify-between border-b border-gray-200 p-5">
              <div>
                <h2 className="text-xl font-semibold">
                  Configured Pack Indices
                </h2>
                <p className="text-sm text-gray-600 mt-1">
                  Drag indices by the handle to set search order. Duplicate pack
                  refs resolve to the first enabled index that contains the ref.
                </p>
              </div>
              <button
                type="button"
                onClick={() => setIsIndexModalOpen(false)}
                className="rounded-lg p-2 text-gray-500 hover:bg-gray-100 hover:text-gray-700"
                aria-label="Close index configuration"
              >
                <X className="w-5 h-5" />
              </button>
            </div>

            <div className="p-5">
              <form onSubmit={handleAddIndex} className="space-y-3 mb-5">
                <input
                  type="text"
                  value={indexForm.name}
                  onChange={(e) =>
                    setIndexForm((prev) => ({ ...prev, name: e.target.value }))
                  }
                  placeholder="Index name"
                  className="w-full px-3 py-2 border border-gray-300 rounded-lg"
                />
                <input
                  type="url"
                  value={indexForm.url}
                  onChange={(e) =>
                    setIndexForm((prev) => ({ ...prev, url: e.target.value }))
                  }
                  placeholder="https://registry.example.com/index.json"
                  className="w-full px-3 py-2 border border-gray-300 rounded-lg"
                  required
                />
                <button
                  type="submit"
                  disabled={createPackIndex.isPending}
                  className="px-4 py-2 bg-blue-600 text-white rounded-lg disabled:opacity-50"
                >
                  Add Index
                </button>
              </form>

              <div className="space-y-2">
                {configuredIndices.map((index, listIndex) => (
                  <div
                    key={index.id}
                    onDragOver={(event) => event.preventDefault()}
                    onDrop={() => handleDropIndex(index.id)}
                    className={`border rounded-lg p-3 text-sm transition-colors ${
                      draggedIndexId === index.id
                        ? "border-blue-300 bg-blue-50"
                        : "border-gray-200 bg-white"
                    }`}
                  >
                    <div className="flex items-start justify-between gap-3">
                      <div className="flex min-w-0 flex-1 items-start gap-3">
                        <button
                          type="button"
                          draggable
                          onDragStart={(event) => {
                            event.dataTransfer.effectAllowed = "move";
                            event.dataTransfer.setData(
                              "text/plain",
                              String(index.id),
                            );
                            setDraggedIndexId(index.id);
                          }}
                          onDragEnd={() => setDraggedIndexId(null)}
                          className="mt-1 rounded p-1 text-gray-400 cursor-grab hover:bg-gray-100 hover:text-gray-600 active:cursor-grabbing"
                          aria-label={`Drag to reorder ${index.name || index.url}`}
                          title="Drag to reorder"
                        >
                          <GripVertical className="h-5 w-5" />
                        </button>
                        <div className="min-w-0">
                          <div className="font-medium">
                            {index.name || index.url}
                          </div>
                          <div className="text-xs text-gray-500">
                            {listIndex === 0
                              ? "Checked first"
                              : "Checked after earlier indices"}
                          </div>
                          <div className="text-gray-500 break-all">
                            {index.url}
                          </div>
                        </div>
                      </div>
                      <div className="flex gap-2">
                        <button
                          type="button"
                          onClick={() =>
                            updatePackIndex.mutate({
                              id: index.id,
                              data: { enabled: !index.enabled },
                            })
                          }
                          className="text-blue-600 hover:text-blue-800"
                        >
                          {index.enabled ? "Disable" : "Enable"}
                        </button>
                        <button
                          type="button"
                          onClick={() => deletePackIndex.mutate(index.id)}
                          className="text-red-600 hover:text-red-800"
                        >
                          Delete
                        </button>
                      </div>
                    </div>
                  </div>
                ))}
                {!packIndices.isLoading &&
                  (packIndices.data?.data || []).length === 0 && (
                    <p className="text-sm text-gray-500">
                      No API-managed indices configured yet.
                    </p>
                  )}
              </div>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
