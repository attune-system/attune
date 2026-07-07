import React, { useState, useEffect, useRef, useCallback } from "react";
import { createPortal } from "react-dom";
import { Link, Outlet, useLocation, useNavigate } from "react-router-dom";
import { useAuth } from "@/contexts/AuthContext";
import {
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  ChevronUp,
  User,
  LogOut,
} from "lucide-react";
import {
  hasAnyPermission,
  requirementsForPath,
  type PermissionRequirement,
} from "@/lib/permissions";
import { navIcons } from "./navIcons";

type UserMenuPosition = {
  top: number;
  left: number;
};

// Color mappings for navigation items — defined outside component for stable reference
const colorClasses = {
  gray: {
    inactive: "text-gray-300 hover:text-white hover:bg-gray-800",
    active: "bg-gray-800 text-white",
    icon: "text-gray-400",
  },
  cyan: {
    inactive: "text-cyan-300 hover:text-cyan-100 hover:bg-cyan-950/30",
    active: "bg-cyan-950/50 text-cyan-100 shadow-lg shadow-cyan-900/50",
    icon: "text-cyan-400",
  },
  blue: {
    inactive: "text-blue-300 hover:text-blue-100 hover:bg-blue-950/30",
    active: "bg-blue-950/50 text-blue-100 shadow-lg shadow-blue-900/50",
    icon: "text-blue-400",
  },
  violet: {
    inactive: "text-violet-300 hover:text-violet-100 hover:bg-violet-950/30",
    active: "bg-violet-950/50 text-violet-100 shadow-lg shadow-violet-900/50",
    icon: "text-violet-400",
  },
  purple: {
    inactive: "text-purple-300 hover:text-purple-100 hover:bg-purple-950/30",
    active: "bg-purple-950/50 text-purple-100 shadow-lg shadow-purple-900/50",
    icon: "text-purple-400",
  },
  fuchsia: {
    inactive: "text-fuchsia-300 hover:text-fuchsia-100 hover:bg-fuchsia-950/30",
    active:
      "bg-fuchsia-950/50 text-fuchsia-100 shadow-lg shadow-fuchsia-900/50",
    icon: "text-fuchsia-400",
  },
  rose: {
    inactive: "text-rose-300 hover:text-rose-100 hover:bg-rose-950/30",
    active: "bg-rose-950/50 text-rose-100 shadow-lg shadow-rose-900/50",
    icon: "text-rose-400",
  },
  orange: {
    inactive: "text-orange-300 hover:text-orange-100 hover:bg-orange-950/30",
    active: "bg-orange-950/50 text-orange-100 shadow-lg shadow-orange-900/50",
    icon: "text-orange-400",
  },
  yellow: {
    inactive: "text-yellow-300 hover:text-yellow-100 hover:bg-yellow-950/30",
    active: "bg-yellow-950/50 text-yellow-100 shadow-lg shadow-yellow-900/50",
    icon: "text-yellow-400",
  },
};

// Navigation sections with dividers and colors
type NavItem = {
  to: string;
  label: string;
  icon: React.ElementType;
  color: string;
  permissions?: PermissionRequirement[];
};

type NavSection = {
  items: NavItem[];
};

const navSections: NavSection[] = [
  {
    items: [
      { to: "/", label: "Dashboard", icon: navIcons.dashboard, color: "gray" },
    ],
  },
  {
    // Component Management - Cool colors (cyan -> blue -> violet)
    items: [
      {
        to: "/actions",
        label: "Actions",
        icon: navIcons.actions,
        color: "cyan",
        permissions: [{ resource: "actions" }],
      },
      {
        to: "/rules",
        label: "Rules",
        icon: navIcons.rules,
        color: "blue",
        permissions: [{ resource: "rules" }],
      },
      {
        to: "/queues",
        label: "Queues",
        icon: navIcons.queues,
        color: "blue",
        permissions: [{ resource: "queues" }],
      },
      {
        to: "/triggers",
        label: "Triggers",
        icon: navIcons.triggers,
        color: "violet",
        permissions: [{ resource: "triggers" }],
      },
      {
        to: "/sensors",
        label: "Sensors",
        icon: navIcons.sensors,
        color: "purple",
        permissions: [{ resource: "sensors" }],
      },
      {
        to: "/policies",
        label: "Policies",
        icon: navIcons.policies,
        color: "purple",
        permissions: [{ resource: "policies" }],
      },
    ],
  },
  {
    // Runtime Logs - Warm colors (fuchsia -> rose -> orange)
    items: [
      {
        to: "/executions",
        label: "Execution History",
        icon: navIcons.executions,
        color: "fuchsia",
        permissions: [{ resource: "executions" }],
      },
      {
        to: "/enforcements",
        label: "Enforcement History",
        icon: navIcons.enforcements,
        color: "rose",
        permissions: [{ resource: "enforcements" }],
      },
      {
        to: "/events",
        label: "Event History",
        icon: navIcons.events,
        color: "orange",
        permissions: [{ resource: "events" }],
      },
      {
        to: "/traces",
        label: "Trace Reports",
        icon: navIcons.traces,
        color: "orange",
        permissions: [{ resource: "executions" }],
      },
      {
        to: "/artifacts",
        label: "Artifacts",
        icon: navIcons.artifacts,
        color: "yellow",
        permissions: [{ resource: "artifacts" }],
      },
    ],
  },
  {
    items: [
      {
        to: "/keys",
        label: "Keys & Secrets",
        icon: navIcons.keys,
        color: "gray",
        permissions: [{ resource: "keys" }],
      },
      {
        to: "/access-control",
        label: "Access Control",
        icon: navIcons.accessControl,
        color: "gray",
        permissions: [{ resource: "identities" }, { resource: "permissions" }],
      },
      {
        to: "/audit-log",
        label: "Audit Log",
        icon: navIcons.auditLog,
        color: "gray",
        permissions: [{ resource: "audit_log" }],
      },
      {
        to: "/retention",
        label: "Runtime Retention",
        icon: navIcons.retention,
        color: "gray",
        permissions: [{ resource: "retention" }],
      },
      {
        to: "/packs",
        label: "Pack Management",
        icon: navIcons.packs,
        color: "gray",
        permissions: [{ resource: "packs" }],
      },
      {
        to: "/runtimes",
        label: "Runtimes & Workers",
        icon: navIcons.runtimes,
        color: "gray",
        permissions: [{ resource: "runtimes" }, { resource: "workers" }],
      },
    ],
  },
];

// NavLink extracted outside MainLayout so React preserves DOM identity across
// re-renders, which is required for CSS transitions to work on collapse/expand.
function NavLink({
  to,
  label,
  icon: Icon,
  color = "gray",
  isCollapsed,
  isActive,
}: {
  to: string;
  label: string;
  icon: React.ElementType;
  color?: string;
  isCollapsed: boolean;
  isActive: boolean;
}) {
  const colors =
    colorClasses[color as keyof typeof colorClasses] || colorClasses.gray;

  return (
    <Link
      to={to}
      className={`flex items-center px-4 py-2 rounded-md transition-colors duration-200 whitespace-nowrap ${
        isActive ? colors.active : colors.inactive
      }`}
      title={isCollapsed ? label : undefined}
    >
      <Icon
        className={`w-5 h-5 flex-shrink-0 ${isActive ? "" : colors.icon}`}
      />
      <span
        className="ml-3 inline-block overflow-hidden transition-all duration-300"
        style={{ maxWidth: isCollapsed ? 0 : "10rem" }}
      >
        {label}
      </span>
    </Link>
  );
}

export default function MainLayout() {
  const { user, logout } = useAuth();
  const location = useLocation();
  const navigate = useNavigate();
  const userMenuButtonRef = useRef<HTMLButtonElement | null>(null);
  const navRef = useRef<HTMLElement | null>(null);
  const navScrollHideTimerRef = useRef<number | null>(null);
  const [isCollapsed, setIsCollapsed] = useState(() => {
    // Initialize from localStorage
    const saved = localStorage.getItem("sidebar-collapsed");
    return saved === "true";
  });
  const [showUserMenu, setShowUserMenu] = useState(false);
  const [isNavScrolling, setIsNavScrolling] = useState(false);
  const [canNavScrollUp, setCanNavScrollUp] = useState(false);
  const [canNavScrollDown, setCanNavScrollDown] = useState(false);
  const [userMenuPosition, setUserMenuPosition] = useState<UserMenuPosition>({
    top: 0,
    left: 0,
  });

  // Persist collapsed state to localStorage and close user menu when expanding
  useEffect(() => {
    localStorage.setItem("sidebar-collapsed", isCollapsed.toString());
  }, [isCollapsed]);

  useEffect(() => {
    const requiredPermissions = requirementsForPath(location.pathname);
    if (
      requiredPermissions &&
      !hasAnyPermission(user, requiredPermissions) &&
      location.pathname !== "/"
    ) {
      navigate("/", { replace: true });
    }
  }, [location.pathname, navigate, user]);

  useEffect(() => {
    if (!showUserMenu || !isCollapsed) {
      return;
    }

    const updateUserMenuPosition = () => {
      const button = userMenuButtonRef.current;
      if (!button) {
        return;
      }

      const rect = button.getBoundingClientRect();
      setUserMenuPosition({
        top: rect.top - 8,
        left: rect.left,
      });
    };

    updateUserMenuPosition();

    window.addEventListener("resize", updateUserMenuPosition);
    window.addEventListener("scroll", updateUserMenuPosition, true);

    return () => {
      window.removeEventListener("resize", updateUserMenuPosition);
      window.removeEventListener("scroll", updateUserMenuPosition, true);
    };
  }, [showUserMenu, isCollapsed]);

  const updateNavOverflowIndicators = useCallback(() => {
    const nav = navRef.current;
    if (!nav) {
      setCanNavScrollUp(false);
      setCanNavScrollDown(false);
      return;
    }

    const tolerance = 2;
    const maxScrollTop = nav.scrollHeight - nav.clientHeight;
    setCanNavScrollUp(nav.scrollTop > tolerance);
    setCanNavScrollDown(maxScrollTop - nav.scrollTop > tolerance);
  }, []);

  useEffect(() => {
    updateNavOverflowIndicators();
    window.addEventListener("resize", updateNavOverflowIndicators);

    return () => {
      window.removeEventListener("resize", updateNavOverflowIndicators);
    };
  }, [isCollapsed, location.pathname, updateNavOverflowIndicators, user]);

  useEffect(() => {
    return () => {
      if (navScrollHideTimerRef.current !== null) {
        window.clearTimeout(navScrollHideTimerRef.current);
      }
    };
  }, []);

  const handleToggleCollapse = () => {
    setIsCollapsed((prev) => {
      const next = !prev;
      if (!next) {
        setShowUserMenu(false);
      }
      return next;
    });
  };

  const handleLogout = () => {
    logout();
  };

  const revealNavScrollbar = () => {
    setIsNavScrolling(true);
    updateNavOverflowIndicators();
    if (navScrollHideTimerRef.current !== null) {
      window.clearTimeout(navScrollHideTimerRef.current);
    }
    navScrollHideTimerRef.current = window.setTimeout(() => {
      setIsNavScrolling(false);
      navScrollHideTimerRef.current = null;
    }, 900);
  };

  return (
    <div className="h-full bg-gray-100 flex overflow-hidden">
      {/* Sidebar */}
      <div
        className={`${
          isCollapsed ? "w-20" : "w-64"
        } bg-gray-900 text-white flex flex-col transition-all duration-300 relative flex-shrink-0 overflow-hidden`}
      >
        {/* Header */}
        <div className="flex items-center justify-center h-20 bg-gray-800 whitespace-nowrap">
          <Link to="/" className="flex items-center justify-center">
            <img
              src="/attune-logo-icon.svg"
              alt="Attune"
              className={`h-14 transition-opacity duration-300 ${isCollapsed ? "opacity-100" : "opacity-0 w-0"}`}
            />
            <img
              src="/attune-logo-navbar.svg"
              alt="Attune"
              className={`h-14 transition-opacity duration-300 ${isCollapsed ? "opacity-0 w-0" : "opacity-100"}`}
            />
          </Link>
        </div>

        {/* Navigation */}
        <div className="relative flex-1 min-h-0">
          <nav
            ref={navRef}
            className={`sidebar-nav-scroll h-full px-4 py-6 overflow-y-auto overflow-x-hidden ${
              isNavScrolling ? "is-scrolling" : ""
            }`}
            onScroll={revealNavScrollbar}
            onWheel={revealNavScrollbar}
            onTouchMove={revealNavScrollbar}
          >
            {navSections
              .map((section) => ({
                ...section,
                items: section.items.filter((item) =>
                  hasAnyPermission(user, item.permissions),
                ),
              }))
              .filter((section) => section.items.length > 0)
              .map((section, sectionIndex, visibleSections) => (
                <div key={sectionIndex}>
                  <div className="space-y-1 mb-3">
                    {section.items.map((item) => {
                      const isActive =
                        location.pathname === item.to ||
                        (item.to !== "/" &&
                          location.pathname.startsWith(item.to));
                      return (
                        <NavLink
                          key={item.to}
                          to={item.to}
                          label={item.label}
                          icon={item.icon}
                          color={item.color}
                          isCollapsed={isCollapsed}
                          isActive={isActive}
                        />
                      );
                    })}
                  </div>
                  {sectionIndex < visibleSections.length - 1 && (
                    <div className="my-3 mx-2 border-t border-gray-700" />
                  )}
                </div>
              ))}
          </nav>
          <div
            aria-hidden="true"
            className={`pointer-events-none absolute inset-x-0 top-0 flex h-9 items-start justify-center bg-gradient-to-b from-gray-900 via-gray-900/80 to-transparent pt-1 transition-opacity duration-200 ${
              canNavScrollUp ? "opacity-100" : "opacity-0"
            }`}
          >
            <ChevronUp className="h-4 w-4 text-gray-400" />
          </div>
          <div
            aria-hidden="true"
            className={`pointer-events-none absolute inset-x-0 bottom-0 flex h-9 items-end justify-center bg-gradient-to-t from-gray-900 via-gray-900/80 to-transparent pb-1 transition-opacity duration-200 ${
              canNavScrollDown ? "opacity-100" : "opacity-0"
            }`}
          >
            <ChevronDown className="h-4 w-4 text-gray-400" />
          </div>
        </div>

        {/* Toggle Button */}
        <div className="px-4 py-3">
          <button
            onClick={handleToggleCollapse}
            className="flex items-center w-full px-3 py-2 text-gray-400 hover:text-white hover:bg-gray-800 rounded-md transition-colors whitespace-nowrap"
            title={isCollapsed ? "Expand sidebar" : "Collapse sidebar"}
          >
            <div className="w-5 h-5 flex-shrink-0 relative">
              <ChevronLeft
                className={`w-5 h-5 absolute inset-0 transition-opacity duration-300 ${
                  isCollapsed ? "opacity-0" : "opacity-100"
                }`}
              />
              <ChevronRight
                className={`w-5 h-5 absolute inset-0 transition-opacity duration-300 ${
                  isCollapsed ? "opacity-100" : "opacity-0"
                }`}
              />
            </div>
            <span
              className="ml-2 inline-block overflow-hidden text-sm transition-all duration-300"
              style={{ maxWidth: isCollapsed ? 0 : "10rem" }}
            >
              Collapse
            </span>
          </button>
        </div>

        {/* User Section */}
        <div className="p-4 bg-gray-800 border-t border-gray-700 overflow-hidden whitespace-nowrap">
          <div className="relative">
            <div className="flex items-center justify-between">
              <div className="flex items-center min-w-0">
                <button
                  onClick={() => isCollapsed && setShowUserMenu(!showUserMenu)}
                  className={`flex-shrink-0 ${isCollapsed ? "cursor-pointer" : "cursor-default"}`}
                  title={user?.login}
                  ref={userMenuButtonRef}
                >
                  <User className="w-5 h-5 text-gray-400" />
                </button>
                <Link
                  to="/profile"
                  className="ml-2 inline-block overflow-hidden transition-all duration-300 min-w-0 rounded-sm font-medium text-sm text-white hover:text-blue-200 focus:outline-none focus:ring-2 focus:ring-blue-400"
                  style={{ maxWidth: isCollapsed ? 0 : "8rem" }}
                  title="User Profile"
                >
                  <span className="block truncate">
                    {user?.display_name || user?.login}
                  </span>
                </Link>
              </div>
              <span
                className="ml-2 inline-block overflow-hidden transition-all duration-300"
                style={{ maxWidth: isCollapsed ? 0 : "2rem" }}
              >
                <button
                  onClick={handleLogout}
                  className="text-gray-400 hover:text-white p-1 flex-shrink-0"
                  title="Logout"
                >
                  <LogOut className="w-5 h-5" />
                </button>
              </span>
            </div>

            {/* User Menu Popup (collapsed mode only) */}
            {isCollapsed &&
              showUserMenu &&
              createPortal(
                <>
                  <div
                    className="fixed inset-0 z-[80]"
                    onClick={() => setShowUserMenu(false)}
                  />
                  <div
                    className="fixed z-[90] w-48 rounded-md border border-gray-700 bg-gray-800 shadow-lg"
                    style={{
                      top: userMenuPosition.top,
                      left: userMenuPosition.left,
                      transform: "translateY(-100%)",
                    }}
                  >
                    <div className="px-4 py-3 border-b border-gray-700">
                      <p className="text-sm font-medium text-white">
                        {user?.display_name || user?.login}
                      </p>
                      {user?.display_name && (
                        <p className="mt-0.5 text-xs text-gray-400">
                          {user?.login}
                        </p>
                      )}
                    </div>
                    <Link
                      to="/profile"
                      onClick={() => setShowUserMenu(false)}
                      className="flex items-center gap-2 px-4 py-2 text-gray-300 hover:bg-gray-700 hover:text-white transition-colors"
                    >
                      <User className="w-4 h-4" />
                      <span>Profile</span>
                    </Link>
                    <button
                      onClick={handleLogout}
                      className="w-full flex items-center gap-2 px-4 py-2 text-left text-gray-300 hover:bg-gray-700 hover:text-white transition-colors"
                    >
                      <LogOut className="w-4 h-4" />
                      <span>Logout</span>
                    </button>
                  </div>
                </>,
                document.body,
              )}
          </div>
        </div>
      </div>

      {/* Main Content */}
      <div className="flex-1 overflow-y-auto">
        <Outlet />
      </div>
    </div>
  );
}
