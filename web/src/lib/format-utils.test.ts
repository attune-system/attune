/**
 * Tests for format utility functions
 */

import { describe, it, expect } from "vitest";
import { labelToRef, extractLocalRef, combineRefs } from "./format-utils";

describe("labelToRef", () => {
  it("converts simple label to ref", () => {
    expect(labelToRef("My Custom Pack")).toBe("my_custom_pack");
  });

  it("converts label with hyphens", () => {
    expect(labelToRef("Alert-on-Error")).toBe("alert_on_error");
  });

  it("converts label with special characters", () => {
    expect(labelToRef("Alert on Error!")).toBe("alert_on_error");
    expect(labelToRef("Test@Pack#123")).toBe("test_pack_123");
  });

  it("handles multiple spaces", () => {
    expect(labelToRef("Notify  User  Action")).toBe("notify_user_action");
  });

  it("removes leading and trailing whitespace", () => {
    expect(labelToRef("  My Pack  ")).toBe("my_pack");
  });

  it("removes leading and trailing underscores", () => {
    expect(labelToRef("_My Pack_")).toBe("my_pack");
  });

  it("replaces consecutive underscores with single underscore", () => {
    expect(labelToRef("My___Pack")).toBe("my_pack");
  });

  it("handles empty string", () => {
    expect(labelToRef("")).toBe("");
  });

  it("handles string with only special characters", () => {
    expect(labelToRef("!@#$%")).toBe("");
  });

  it("preserves numbers", () => {
    expect(labelToRef("Pack 123 Version")).toBe("pack_123_version");
  });

  it("handles camelCase", () => {
    expect(labelToRef("myCustomPack")).toBe("mycustompack");
  });

  it("handles mixed case with spaces", () => {
    expect(labelToRef("My Custom PACK")).toBe("my_custom_pack");
  });

  it("handles dots and slashes", () => {
    expect(labelToRef("my.pack/action")).toBe("my_pack_action");
  });

  it("handles parentheses and brackets", () => {
    expect(labelToRef("Pack (Production) [v2]")).toBe("pack_production_v2");
  });
});

describe("extractLocalRef", () => {
  it("extracts local ref from full ref with one dot", () => {
    expect(extractLocalRef("core.timer")).toBe("timer");
  });

  it("extracts local ref from full ref with multiple dots", () => {
    expect(extractLocalRef("mypack.sub.my_rule")).toBe("my_rule");
  });

  it("returns the same ref if no dot is present", () => {
    expect(extractLocalRef("simple_ref")).toBe("simple_ref");
  });

  it("handles empty string", () => {
    expect(extractLocalRef("")).toBe("");
  });

  it("handles ref ending with dot", () => {
    expect(extractLocalRef("mypack.")).toBe("");
  });

  it("handles ref starting with dot", () => {
    expect(extractLocalRef(".localref")).toBe("localref");
  });

  it("extracts from complex nested ref", () => {
    expect(extractLocalRef("company.team.project.action")).toBe("action");
  });
});

describe("combineRefs", () => {
  it("combines pack ref and local ref", () => {
    expect(combineRefs("mypack", "my_rule")).toBe("mypack.my_rule");
  });

  it("combines with simple refs", () => {
    expect(combineRefs("core", "timer")).toBe("core.timer");
  });

  it("handles empty pack ref", () => {
    expect(combineRefs("", "localref")).toBe(".localref");
  });

  it("handles empty local ref", () => {
    expect(combineRefs("mypack", "")).toBe("mypack.");
  });

  it("handles both empty", () => {
    expect(combineRefs("", "")).toBe(".");
  });

  it("combines with underscores in refs", () => {
    expect(combineRefs("my_pack", "my_rule")).toBe("my_pack.my_rule");
  });

  it("combines with numbers in refs", () => {
    expect(combineRefs("pack123", "rule456")).toBe("pack123.rule456");
  });
});

describe("integration tests", () => {
  it("label to ref to combined ref workflow", () => {
    const label = "My Alert Rule";
    const packRef = "alerts";

    const localRef = labelToRef(label);
    expect(localRef).toBe("my_alert_rule");

    const fullRef = combineRefs(packRef, localRef);
    expect(fullRef).toBe("alerts.my_alert_rule");

    const extractedLocal = extractLocalRef(fullRef);
    expect(extractedLocal).toBe("my_alert_rule");
  });

  it("handles complex label transformation workflow", () => {
    const label = "Production Alert (Critical!)";
    const packRef = "monitoring";

    const localRef = labelToRef(label);
    expect(localRef).toBe("production_alert_critical");

    const fullRef = combineRefs(packRef, localRef);
    expect(fullRef).toBe("monitoring.production_alert_critical");
  });

  it("round-trip with existing full ref", () => {
    const fullRef = "mypack.existing_rule";

    const localRef = extractLocalRef(fullRef);
    expect(localRef).toBe("existing_rule");

    const reconstructed = combineRefs("mypack", localRef);
    expect(reconstructed).toBe(fullRef);
  });
});
