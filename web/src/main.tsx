// import { StrictMode } from "react"; // Disabled - see comment below
import { createRoot } from "react-dom/client";
import "./index.css";
import "./lib/api-config";
import { initializeApiWrapper } from "./lib/api-wrapper";
import { InstallationSettingsProvider } from "./contexts/InstallationSettingsContext";
import App from "./App";

// Initialize API wrapper for token refresh
initializeApiWrapper();

// StrictMode disabled to prevent duplicate WebSocket connections in development
// React 18's StrictMode intentionally double-mounts components to detect side effects,
// which causes two WebSocket connections to be briefly created. This is expected behavior
// in development but can be confusing. Re-enable for production if needed.
createRoot(document.getElementById("root")!).render(
  <InstallationSettingsProvider>
    <App />
  </InstallationSettingsProvider>,
);
