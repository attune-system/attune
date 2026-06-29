import { FormEvent, useEffect, useState } from "react";
import { AlertCircle, CheckCircle2, KeyRound, Save, User } from "lucide-react";
import { ApiError, AuthService } from "@/api";
import type { ProviderProfileResponse } from "@/api";
import { useAuth } from "@/contexts/AuthContext";

type ProviderProfile = ProviderProfileResponse;

function apiErrorMessage(error: unknown, fallback: string): string {
  if (error instanceof ApiError) {
    const bodyMessage =
      typeof error.body?.error?.message === "string"
        ? error.body.error.message
        : typeof error.body?.message === "string"
          ? error.body.message
          : null;
    return bodyMessage ?? error.message ?? fallback;
  }

  return error instanceof Error ? error.message : fallback;
}

function formatProvider(provider: string): string {
  return provider === "oidc"
    ? "OIDC / SSO"
    : provider === "ldap"
      ? "LDAP"
      : provider.toUpperCase();
}

function ProviderField({
  label,
  value,
}: {
  label: string;
  value: string | boolean | null | undefined;
}) {
  if (value === null || value === undefined || value === "") {
    return null;
  }

  return (
    <div>
      <dt className="text-sm font-medium text-gray-500">{label}</dt>
      <dd className="mt-1 break-words text-sm text-gray-900">
        {typeof value === "boolean" ? (value ? "Yes" : "No") : value}
      </dd>
    </div>
  );
}

function ProviderProfileSummary({
  providerProfile,
  fallbackLogin,
  fallbackDisplayName,
}: {
  providerProfile: ProviderProfile | null | undefined;
  fallbackLogin: string | undefined;
  fallbackDisplayName: string | null | undefined;
}) {
  const provider = providerProfile?.provider ?? "sso";
  const groups = providerProfile?.groups ?? [];

  return (
    <section className="rounded-lg bg-white p-6 shadow">
      <div className="mb-5 flex items-center gap-3">
        <div className="rounded-full bg-blue-100 p-2">
          <User className="h-5 w-5 text-blue-600" />
        </div>
        <div>
          <h2 className="text-lg font-semibold text-gray-900">
            Provider profile
          </h2>
          <p className="text-sm text-gray-500">
            Read-only user information supplied by {formatProvider(provider)}.
          </p>
        </div>
      </div>

      <dl className="grid gap-4 sm:grid-cols-2">
        <ProviderField label="Provider" value={formatProvider(provider)} />
        <ProviderField
          label="Display Name"
          value={providerProfile?.display_name ?? fallbackDisplayName}
        />
        <ProviderField
          label="Login"
          value={providerProfile?.login ?? fallbackLogin}
        />
        <ProviderField label="Email" value={providerProfile?.email} />
        <ProviderField
          label="Email Verified"
          value={providerProfile?.email_verified}
        />
        <ProviderField label="Issuer" value={providerProfile?.issuer} />
        <ProviderField label="Subject" value={providerProfile?.subject} />
        <ProviderField
          label="Distinguished Name"
          value={providerProfile?.distinguished_name}
        />
      </dl>

      <div className="mt-6">
        <h3 className="text-sm font-medium text-gray-500">Groups</h3>
        {groups.length > 0 ? (
          <div className="mt-2 flex flex-wrap gap-2">
            {groups.map((group) => (
              <span
                key={group}
                className="rounded-full bg-gray-100 px-3 py-1 text-sm text-gray-700"
              >
                {group}
              </span>
            ))}
          </div>
        ) : (
          <p className="mt-1 text-sm text-gray-500">
            No provider groups were supplied for this identity.
          </p>
        )}
      </div>
    </section>
  );
}

export default function UserProfilePage() {
  const { user, refreshUser } = useAuth();
  const [displayName, setDisplayName] = useState(user?.display_name ?? "");
  const [profileMessage, setProfileMessage] = useState<string | null>(null);
  const [profileError, setProfileError] = useState<string | null>(null);
  const [isSavingProfile, setIsSavingProfile] = useState(false);
  const [passwordForm, setPasswordForm] = useState({
    currentPassword: "",
    newPassword: "",
    confirmPassword: "",
  });
  const [passwordMessage, setPasswordMessage] = useState<string | null>(null);
  const [passwordError, setPasswordError] = useState<string | null>(null);
  const [isChangingPassword, setIsChangingPassword] = useState(false);

  const isLocal = user?.is_local === true;
  const canChangePassword = user?.can_change_password === true;
  const authProvider = user?.auth_provider ?? "local";
  const providerProfile = user?.provider_profile;

  useEffect(() => {
    setDisplayName(user?.display_name ?? "");
  }, [user?.display_name]);

  const handleProfileSubmit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    setProfileMessage(null);
    setProfileError(null);

    if (!isLocal) {
      setProfileError("Profile details are managed by your identity provider.");
      return;
    }

    setIsSavingProfile(true);
    try {
      await AuthService.updateCurrentUser({
        requestBody: {
          display_name: displayName,
        },
      });
      await refreshUser();
      setProfileMessage("Profile updated.");
    } catch (error) {
      setProfileError(apiErrorMessage(error, "Failed to update profile."));
    } finally {
      setIsSavingProfile(false);
    }
  };

  const handlePasswordSubmit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    setPasswordMessage(null);
    setPasswordError(null);

    if (!canChangePassword) {
      setPasswordError("Password changes are only available for local users.");
      return;
    }

    if (passwordForm.newPassword !== passwordForm.confirmPassword) {
      setPasswordError("New password and confirmation do not match.");
      return;
    }

    setIsChangingPassword(true);
    try {
      await AuthService.changePassword({
        requestBody: {
          current_password: passwordForm.currentPassword,
          new_password: passwordForm.newPassword,
        },
      });
      setPasswordForm({
        currentPassword: "",
        newPassword: "",
        confirmPassword: "",
      });
      setPasswordMessage("Password changed.");
    } catch (error) {
      setPasswordError(apiErrorMessage(error, "Failed to change password."));
    } finally {
      setIsChangingPassword(false);
    }
  };

  return (
    <div className="mx-auto max-w-4xl p-6">
      <div className="mb-6">
        <h1 className="text-3xl font-bold text-gray-900">User Profile</h1>
        <p className="mt-2 text-gray-600">
          Manage your Attune profile details and local account password.
        </p>
      </div>

      {!isLocal && (
        <div className="mb-6 rounded-lg border border-blue-200 bg-blue-50 p-4">
          <div className="flex items-start gap-3">
            <AlertCircle className="mt-0.5 h-5 w-5 flex-shrink-0 text-blue-600" />
            <div className="text-sm text-blue-900">
              <p className="font-medium">Provider-managed account</p>
              <p className="mt-1">
                This account is managed by {authProvider.toUpperCase()}. Update
                your display name or password through that provider.
              </p>
            </div>
          </div>
        </div>
      )}

      {!isLocal ? (
        <ProviderProfileSummary
          providerProfile={providerProfile}
          fallbackLogin={user?.login}
          fallbackDisplayName={user?.display_name}
        />
      ) : (
        <div className="space-y-6">
          <section className="rounded-lg bg-white p-6 shadow">
            <div className="mb-5 flex items-center gap-3">
              <div className="rounded-full bg-blue-100 p-2">
                <User className="h-5 w-5 text-blue-600" />
              </div>
              <div>
                <h2 className="text-lg font-semibold text-gray-900">
                  Profile details
                </h2>
                <p className="text-sm text-gray-500">
                  Signed in as {user?.login}
                </p>
              </div>
            </div>

            <form onSubmit={handleProfileSubmit} className="space-y-4">
              <div>
                <label
                  htmlFor="display-name"
                  className="block text-sm font-medium text-gray-700"
                >
                  Display Name
                </label>
                <input
                  id="display-name"
                  type="text"
                  value={displayName}
                  onChange={(event) => setDisplayName(event.target.value)}
                  disabled={!isLocal || isSavingProfile}
                  maxLength={255}
                  className="mt-1 block w-full rounded-md border border-gray-300 px-3 py-2 shadow-sm focus:border-blue-500 focus:outline-none focus:ring-blue-500 disabled:cursor-not-allowed disabled:bg-gray-100"
                  placeholder="Optional display name"
                />
              </div>

              {profileError && (
                <div className="flex items-center gap-2 rounded-md border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-700">
                  <AlertCircle className="h-4 w-4" />
                  {profileError}
                </div>
              )}
              {profileMessage && (
                <div className="flex items-center gap-2 rounded-md border border-green-200 bg-green-50 px-3 py-2 text-sm text-green-700">
                  <CheckCircle2 className="h-4 w-4" />
                  {profileMessage}
                </div>
              )}

              <button
                type="submit"
                disabled={!isLocal || isSavingProfile}
                className="inline-flex items-center gap-2 rounded-lg bg-blue-600 px-4 py-2 text-sm font-medium text-white transition-colors hover:bg-blue-700 disabled:cursor-not-allowed disabled:opacity-50"
              >
                <Save className="h-4 w-4" />
                {isSavingProfile ? "Saving..." : "Save Profile"}
              </button>
            </form>
          </section>

          <section className="rounded-lg bg-white p-6 shadow">
            <div className="mb-5 flex items-center gap-3">
              <div className="rounded-full bg-violet-100 p-2">
                <KeyRound className="h-5 w-5 text-violet-600" />
              </div>
              <div>
                <h2 className="text-lg font-semibold text-gray-900">
                  Reset password
                </h2>
                <p className="text-sm text-gray-500">
                  Change the password used for local Attune login.
                </p>
              </div>
            </div>

            <form onSubmit={handlePasswordSubmit} className="space-y-4">
              <div>
                <label
                  htmlFor="current-password"
                  className="block text-sm font-medium text-gray-700"
                >
                  Current Password
                </label>
                <input
                  id="current-password"
                  type="password"
                  value={passwordForm.currentPassword}
                  onChange={(event) =>
                    setPasswordForm((current) => ({
                      ...current,
                      currentPassword: event.target.value,
                    }))
                  }
                  disabled={!canChangePassword || isChangingPassword}
                  required={canChangePassword}
                  className="mt-1 block w-full rounded-md border border-gray-300 px-3 py-2 shadow-sm focus:border-blue-500 focus:outline-none focus:ring-blue-500 disabled:cursor-not-allowed disabled:bg-gray-100"
                />
              </div>

              <div className="grid gap-4 sm:grid-cols-2">
                <div>
                  <label
                    htmlFor="new-password"
                    className="block text-sm font-medium text-gray-700"
                  >
                    New Password
                  </label>
                  <input
                    id="new-password"
                    type="password"
                    value={passwordForm.newPassword}
                    onChange={(event) =>
                      setPasswordForm((current) => ({
                        ...current,
                        newPassword: event.target.value,
                      }))
                    }
                    disabled={!canChangePassword || isChangingPassword}
                    required={canChangePassword}
                    minLength={8}
                    maxLength={128}
                    className="mt-1 block w-full rounded-md border border-gray-300 px-3 py-2 shadow-sm focus:border-blue-500 focus:outline-none focus:ring-blue-500 disabled:cursor-not-allowed disabled:bg-gray-100"
                  />
                </div>

                <div>
                  <label
                    htmlFor="confirm-password"
                    className="block text-sm font-medium text-gray-700"
                  >
                    Confirm New Password
                  </label>
                  <input
                    id="confirm-password"
                    type="password"
                    value={passwordForm.confirmPassword}
                    onChange={(event) =>
                      setPasswordForm((current) => ({
                        ...current,
                        confirmPassword: event.target.value,
                      }))
                    }
                    disabled={!canChangePassword || isChangingPassword}
                    required={canChangePassword}
                    minLength={8}
                    maxLength={128}
                    className="mt-1 block w-full rounded-md border border-gray-300 px-3 py-2 shadow-sm focus:border-blue-500 focus:outline-none focus:ring-blue-500 disabled:cursor-not-allowed disabled:bg-gray-100"
                  />
                </div>
              </div>

              {passwordError && (
                <div className="flex items-center gap-2 rounded-md border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-700">
                  <AlertCircle className="h-4 w-4" />
                  {passwordError}
                </div>
              )}
              {passwordMessage && (
                <div className="flex items-center gap-2 rounded-md border border-green-200 bg-green-50 px-3 py-2 text-sm text-green-700">
                  <CheckCircle2 className="h-4 w-4" />
                  {passwordMessage}
                </div>
              )}

              <button
                type="submit"
                disabled={!canChangePassword || isChangingPassword}
                className="inline-flex items-center gap-2 rounded-lg bg-violet-600 px-4 py-2 text-sm font-medium text-white transition-colors hover:bg-violet-700 disabled:cursor-not-allowed disabled:opacity-50"
              >
                <KeyRound className="h-4 w-4" />
                {isChangingPassword ? "Changing..." : "Change Password"}
              </button>
            </form>
          </section>
        </div>
      )}
    </div>
  );
}
