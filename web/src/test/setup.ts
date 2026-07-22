import "@testing-library/jest-dom/vitest";
import { afterEach } from "vitest";
import { cleanup } from "@testing-library/react";

// Vitest doesn't run in Jest's "globals" mode here, so Testing Library's
// automatic afterEach cleanup registration doesn't kick in on its own —
// register it explicitly so each test starts from an empty DOM.
afterEach(() => {
  cleanup();
});
