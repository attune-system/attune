import js from "@eslint/js";
import globals from "globals";
import reactHooks from "eslint-plugin-react-hooks";
import reactRefresh from "eslint-plugin-react-refresh";
import tseslint from "typescript-eslint";
import { defineConfig, globalIgnores } from "eslint/config";

export default defineConfig([
  globalIgnores(["dist", "src/api"]),
  {
    files: ["**/*.{ts,tsx}"],
    extends: [
      js.configs.recommended,
      tseslint.configs.recommended,
      reactHooks.configs.flat.recommended,
      reactRefresh.configs.vite,
    ],
    languageOptions: {
      ecmaVersion: 2020,
      globals: globals.browser,
    },
    rules: {
      "no-restricted-syntax": [
        "error",
        {
          selector:
            "TSAsExpression[typeAnnotation.type='TSTypeReference'][typeAnnotation.typeName.name=/Request$/]",
          message:
            "Do not cast API request bodies. Construct an explicitly typed request or use satisfies.",
        },
        {
          selector:
            "TSAsExpression[typeAnnotation.type='TSIndexedAccessType'][typeAnnotation.objectType.typeName.name=/Request$/]",
          message:
            "Do not cast API request fields. Correct the generated contract or validate the value.",
        },
      ],
    },
  },
]);
