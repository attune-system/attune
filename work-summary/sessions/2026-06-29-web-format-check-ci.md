# 2026-06-29 — Web format check CI wiring

## Summary
Added the web formatter check to the blocking GitHub Actions workflow so JavaScript/TypeScript formatting is enforced alongside the existing web lint/typecheck/build checks.

## Changes
- Updated `/.github/workflows/ci.yml`
  - Added a `Prettier` step to the `web-blocking` job
  - The job now runs `npm run format:check` after `npm ci` and before ESLint/typecheck/build
- Updated `AGENTS.md`
  - Documented that the web UI uses Prettier formatting scripts
  - Noted that the `web-blocking` CI job enforces `format:check`

## Notes
- This CI change assumes the web workspace changes that add Prettier and the `format:check` script are included with the same branch/commit series.
