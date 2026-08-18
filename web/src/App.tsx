import { lazy, Suspense } from "react";
import { BrowserRouter, Routes, Route, Navigate } from "react-router-dom";
import { QueryClientProvider } from "@tanstack/react-query";
import { AuthProvider } from "@/contexts/AuthContext";
import { WebSocketProvider } from "@/contexts/WebSocketContext";
import { queryClient } from "@/lib/query-client";
import ProtectedRoute from "@/components/common/ProtectedRoute";
import MainLayout from "@/components/layout/MainLayout";
import { QueueDetailPage } from "@/pages/queues/QueueDetailPage";

// Lazy-loaded page components for code splitting
const LoginPage = lazy(() => import("@/pages/auth/LoginPage"));
const OidcCallbackPage = lazy(() => import("@/pages/auth/OidcCallbackPage"));
const DashboardPage = lazy(() => import("@/pages/dashboard/DashboardPage"));
const DashboardEditorPage = lazy(
  () => import("@/pages/dashboard/DashboardEditorPage"),
);
const UserProfilePage = lazy(() => import("@/pages/profile/UserProfilePage"));
const PacksPage = lazy(() => import("@/pages/packs/PacksPage"));
const PackCreatePage = lazy(() => import("@/pages/packs/PackCreatePage"));
const PackInstallPage = lazy(() => import("@/pages/packs/PackInstallPage"));
const PackEditPage = lazy(() => import("@/pages/packs/PackEditPage"));
const PackTestDetailPage = lazy(
  () => import("@/pages/packs/PackTestDetailPage"),
);
const ActionsPage = lazy(() => import("@/pages/actions/ActionsPage"));
const RuntimesPage = lazy(() => import("@/pages/runtimes/RuntimesPage"));
const WorkflowBuilderPage = lazy(
  () => import("@/pages/actions/WorkflowBuilderPage"),
);
const RulesPage = lazy(() => import("@/pages/rules/RulesPage"));
const RuleCreatePage = lazy(() => import("@/pages/rules/RuleCreatePage"));
const RuleEditPage = lazy(() => import("@/pages/rules/RuleEditPage"));
const ExecutionsPage = lazy(() => import("@/pages/executions/ExecutionsPage"));
const ExecutionDetailPage = lazy(
  () => import("@/pages/executions/ExecutionDetailPage"),
);
const ArtifactsPage = lazy(() => import("@/pages/artifacts/ArtifactsPage"));
const ArtifactDetailPage = lazy(
  () => import("@/pages/artifacts/ArtifactDetailPage"),
);
const QueuesPage = lazy(() => import("@/pages/queues/QueuesPage"));
const QueueCreatePage = lazy(() => import("@/pages/queues/QueueCreatePage"));
const QueueEditPage = lazy(() => import("@/pages/queues/QueueEditPage"));
const PoliciesPage = lazy(() => import("@/pages/policies/PoliciesPage"));
const PolicyCreatePage = lazy(
  () => import("@/pages/policies/PolicyCreatePage"),
);
const PolicyDetailPage = lazy(
  () => import("@/pages/policies/PolicyDetailPage"),
);
const PolicyEditPage = lazy(() => import("@/pages/policies/PolicyEditPage"));
const EventsPage = lazy(() => import("@/pages/events/EventsPage"));
const EventDetailPage = lazy(() => import("@/pages/events/EventDetailPage"));
const EnforcementsPage = lazy(
  () => import("@/pages/enforcements/EnforcementsPage"),
);
const TraceReportPage = lazy(() => import("@/pages/traces/TraceReportPage"));
const EnforcementDetailPage = lazy(
  () => import("@/pages/enforcements/EnforcementDetailPage"),
);
const KeysPage = lazy(() => import("@/pages/keys/KeysPage"));
const CachesPage = lazy(() => import("@/pages/caches/CachesPage"));
const CacheNamespaceDetailPage = lazy(
  () => import("@/pages/caches/CacheNamespaceDetailPage"),
);
const AuditLogPage = lazy(() => import("@/pages/audit/AuditLogPage"));
const RetentionConfigPage = lazy(
  () => import("@/pages/retention/RetentionConfigPage"),
);
const TriggersPage = lazy(() => import("@/pages/triggers/TriggersPage"));
const TriggerCreatePage = lazy(
  () => import("@/pages/triggers/TriggerCreatePage"),
);
const TriggerEditPage = lazy(() => import("@/pages/triggers/TriggerEditPage"));
const SensorsPage = lazy(() => import("@/pages/sensors/SensorsPage"));
const AccessControlPage = lazy(
  () => import("@/pages/access-control/AccessControlPage"),
);
const IdentityDetailPage = lazy(
  () => import("@/pages/access-control/IdentityDetailPage"),
);
const PermissionSetDetailPage = lazy(
  () => import("@/pages/access-control/PermissionSetDetailPage"),
);

function PageLoader() {
  return (
    <div className="flex items-center justify-center h-64">
      <div className="text-center">
        <div className="animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600 mx-auto"></div>
        <p className="mt-3 text-sm text-gray-500">Loading…</p>
      </div>
    </div>
  );
}

function App() {
  return (
    <QueryClientProvider client={queryClient}>
      <AuthProvider>
        <WebSocketProvider>
          <BrowserRouter>
            <Suspense fallback={<PageLoader />}>
              <Routes>
                {/* Public routes */}
                <Route path="/login" element={<LoginPage />} />
                <Route path="/login/callback" element={<OidcCallbackPage />} />

                {/* Protected routes */}
                <Route
                  path="/"
                  element={
                    <ProtectedRoute>
                      <MainLayout />
                    </ProtectedRoute>
                  }
                >
                  <Route index element={<DashboardPage />} />
                  <Route
                    path="dashboards/new"
                    element={<DashboardEditorPage />}
                  />
                  <Route
                    path="dashboards/:ref/edit"
                    element={<DashboardEditorPage />}
                  />
                  <Route path="profile" element={<UserProfilePage />} />
                  <Route path="packs" element={<PacksPage />} />
                  <Route path="packs/new" element={<PackCreatePage />} />
                  <Route path="packs/install" element={<PackInstallPage />} />
                  <Route path="packs/tests/:id" element={<PackTestDetailPage />} />
                  <Route path="packs/:ref" element={<PacksPage />} />
                  <Route path="packs/:ref/edit" element={<PackEditPage />} />
                  <Route path="actions" element={<ActionsPage />} />
                  <Route path="runtimes" element={<RuntimesPage />} />
                  <Route path="runtimes/new" element={<RuntimesPage />} />
                  <Route path="runtimes/:ref" element={<RuntimesPage />} />
                  <Route
                    path="actions/workflows/new"
                    element={<WorkflowBuilderPage />}
                  />
                  <Route
                    path="actions/workflows/:ref/edit"
                    element={<WorkflowBuilderPage />}
                  />
                  <Route path="actions/:ref" element={<ActionsPage />} />
                  <Route path="rules" element={<RulesPage />} />
                  <Route path="rules/new" element={<RuleCreatePage />} />
                  <Route path="rules/:ref" element={<RulesPage />} />
                  <Route path="rules/:ref/edit" element={<RuleEditPage />} />
                  <Route path="executions" element={<ExecutionsPage />} />
                  <Route
                    path="executions/:id"
                    element={<ExecutionDetailPage />}
                  />
                  <Route path="artifacts" element={<ArtifactsPage />} />
                  <Route
                    path="artifacts/:id"
                    element={<ArtifactDetailPage />}
                  />
                  <Route path="queues" element={<QueuesPage />} />
                  <Route path="queues/new" element={<QueueCreatePage />} />
                  <Route path="queues/:ref" element={<QueueDetailPage />} />
                  <Route path="queues/:ref/edit" element={<QueueEditPage />} />
                  <Route path="policies" element={<PoliciesPage />} />
                  <Route path="policies/new" element={<PolicyCreatePage />} />
                  <Route path="policies/:ref" element={<PolicyDetailPage />} />
                  <Route
                    path="policies/:ref/edit"
                    element={<PolicyEditPage />}
                  />
                  <Route path="events" element={<EventsPage />} />
                  <Route path="events/:id" element={<EventDetailPage />} />
                  <Route path="enforcements" element={<EnforcementsPage />} />
                  <Route path="traces" element={<TraceReportPage />} />
                  <Route
                    path="enforcements/:id"
                    element={<EnforcementDetailPage />}
                  />
                  <Route path="keys" element={<KeysPage />} />
                  <Route path="caches" element={<CachesPage />} />
                  <Route
                    path="caches/:ownerType/:ownerRef/:namespace"
                    element={<CacheNamespaceDetailPage />}
                  />
                  <Route path="triggers" element={<TriggersPage />} />
                  <Route
                    path="triggers/create"
                    element={<TriggerCreatePage />}
                  />
                  <Route path="triggers/:ref" element={<TriggersPage />} />
                  <Route
                    path="triggers/:ref/edit"
                    element={<TriggerEditPage />}
                  />
                  <Route path="sensors" element={<SensorsPage />} />
                  <Route path="sensors/:ref" element={<SensorsPage />} />
                  <Route
                    path="access-control"
                    element={<AccessControlPage />}
                  />
                  <Route
                    path="access-control/identities/:id"
                    element={<IdentityDetailPage />}
                  />
                  <Route
                    path="access-control/permission-sets/:ref"
                    element={<PermissionSetDetailPage />}
                  />
                  <Route path="audit-log" element={<AuditLogPage />} />
                  <Route path="retention" element={<RetentionConfigPage />} />
                </Route>

                {/* Catch all - redirect to dashboard */}
                <Route path="*" element={<Navigate to="/" replace />} />
              </Routes>
            </Suspense>
          </BrowserRouter>
        </WebSocketProvider>
      </AuthProvider>
    </QueryClientProvider>
  );
}

export default App;
