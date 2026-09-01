/**
 * Utility functions for formatting and converting values
 */

export function formatJsonValue(value: unknown, indentation?: number): string {
  if (typeof value === "string") return value;
  if (value === undefined) return "";
  if (typeof value !== "object") return String(value);
  return JSON.stringify(value, null, indentation) ?? "";
}

/**
 * Convert a label string to a ref-compatible format
 * - Converts to lowercase
 * - Replaces spaces and special characters with underscores
 * - Removes consecutive underscores
 * - Trims leading/trailing underscores
 *
 * @param label - The label string to convert
 * @returns A ref-compatible string
 *
 * @example
 * labelToRef("My Custom Pack") // "my_custom_pack"
 * labelToRef("Alert-on-Error!") // "alert_on_error"
 * labelToRef("  Notify  User  ") // "notify_user"
 */
export function labelToRef(label: string): string {
  return label
    .toLowerCase() // Convert to lowercase
    .trim() // Remove leading/trailing whitespace
    .replace(/[^a-z0-9]+/g, "_") // Replace non-alphanumeric chars with underscore
    .replace(/^_+|_+$/g, "") // Remove leading/trailing underscores
    .replace(/_+/g, "_"); // Replace consecutive underscores with single underscore
}

/**
 * Extract the local part of a ref (after the pack prefix)
 *
 * @param fullRef - The full ref string (e.g., "mypack.my_rule")
 * @param packRef - Optional pack reference to remove as prefix (e.g., "mypack")
 * @returns The local part after the pack prefix (e.g., "my_rule")
 *
 * @example
 * extractLocalRef("core.timer") // "timer"
 * extractLocalRef("mypack.my_rule") // "my_rule"
 * extractLocalRef("mypack.my_rule", "mypack") // "my_rule"
 * extractLocalRef("simple_ref") // "simple_ref"
 */
export function extractLocalRef(fullRef: string, packRef?: string): string {
  if (packRef && fullRef.startsWith(`${packRef}.`)) {
    return fullRef.substring(packRef.length + 1);
  }
  const lastDotIndex = fullRef.lastIndexOf(".");
  return lastDotIndex >= 0 ? fullRef.substring(lastDotIndex + 1) : fullRef;
}

/**
 * Combine pack ref and local ref into full ref
 *
 * @param packRef - The pack reference (e.g., "mypack")
 * @param localRef - The local reference (e.g., "my_rule")
 * @returns The combined ref (e.g., "mypack.my_rule")
 *
 * @example
 * combineRefs("mypack", "my_rule") // "mypack.my_rule"
 * combineRefs("core", "timer") // "core.timer"
 */
export function combineRefs(packRef: string, localRef: string): string {
  return `${packRef}.${localRef}`;
}

/**
 * Combine pack ref and local ref into full ref (alias for combineRefs)
 *
 * @param packRef - The pack reference (e.g., "mypack")
 * @param localRef - The local reference (e.g., "my_rule")
 * @returns The combined ref (e.g., "mypack.my_rule")
 *
 * @example
 * combinePackLocalRef("mypack", "my_rule") // "mypack.my_rule"
 * combinePackLocalRef("core", "timer") // "core.timer"
 */
export function combinePackLocalRef(packRef: string, localRef: string): string {
  return combineRefs(packRef, localRef);
}
