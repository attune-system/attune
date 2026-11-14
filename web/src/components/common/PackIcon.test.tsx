import { fireEvent, render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import PackIcon from "./PackIcon";

vi.mock("@/api", () => ({
  OpenAPI: { BASE: "https://attune.example" },
}));

afterEach(() => vi.unstubAllGlobals());

describe("PackIcon content containment", () => {
  it("uses the public HTTP response URL so document navigation retains its CSP", () => {
    const fetch = vi.fn();
    vi.stubGlobal("fetch", fetch);
    const { container } = render(<PackIcon packRef=" untrusted_pack " />);
    const image = container.querySelector("img");

    expect(image).toHaveAttribute(
      "src",
      "https://attune.example/api/v1/packs/untrusted_pack/icon",
    );
    // Do not download/repackage an SVG into an app-origin blob document.
    expect(fetch).not.toHaveBeenCalled();
  });

  it("keeps the fallback for missing, invalid or undecodable images", () => {
    const { container } = render(<PackIcon packRef="missing" title="Pack" />);
    fireEvent.error(container.querySelector("img")!);

    expect(container.querySelector("img")).toBeNull();
    expect(container.querySelector("svg")).not.toBeNull();
    expect(container.firstChild).toHaveAttribute("title", "Pack");
  });

  it("loads a different pack after an image failure", () => {
    const { container, rerender } = render(<PackIcon packRef="missing" />);
    fireEvent.error(container.querySelector("img")!);
    rerender(<PackIcon packRef="ordinary" />);

    expect(container.querySelector("img")).toHaveAttribute(
      "src",
      "https://attune.example/api/v1/packs/ordinary/icon",
    );
  });

  it.each([undefined, null, "", "   "])(
    "shows the fallback when the pack reference is %s",
    (packRef) => {
      const { container } = render(<PackIcon packRef={packRef} />);
      expect(container.querySelector("img")).toBeNull();
      expect(container.querySelector("svg")).not.toBeNull();
    },
  );

  it("encodes the pack reference within the HTTP path", () => {
    const { container } = render(<PackIcon packRef="bad/ref?#" />);
    expect(container.querySelector("img")).toHaveAttribute(
      "src",
      "https://attune.example/api/v1/packs/bad%2Fref%3F%23/icon",
    );
  });
});
