// secrets.nonprod.cue — the single committed SOURCE OF TRUTH for the
// non-production values of every Encore secret declared in
// apps/api/auth/secrets.go (`var secrets struct`).
//
// WHY THIS FILE EXISTS (bead unblock-f6z, reverses tv8.67)
// -------------------------------------------------------------------------
// Secrets used to live in two uncoordinated registries with divergent
// naming and no drift enforcement:
//   1. Encore Platform secrets (Go-identifier names, e.g. GitHubOAuthRedirectURI),
//      set via `encore secret set`.
//   2. GitHub Actions repo secrets (CI_ prefix + SCREAMING_SNAKE, e.g.
//      CI_GITHUB_OAUTH_REDIRECT_URI), consumed by apps-api-ci.yml which
//      fabricated .secrets.local.cue at CI time with a `${VAR:-default}`
//      fallback that MASKED a missing GitHub secret (CI stayed green while
//      the platform fail-fast only surfaced at deploy). The 5th secret
//      (GitHubOAuthRedirectURI) was added to code + spec + local but never
//      provisioned on the platform OR in GitHub — the mask hid it until a
//      deploy panicked (`auth: GitHubOAuthRedirectURI is empty`,
//      apps/api/auth/secrets.go:138).
//
// DECISION (orchestrator + Miguel, 2026-06-03 — OPTION A): non-prod
// placeholders are NOT real secrets — they are fake, well-shaped values the
// local emulator and CI need only to be non-empty. Commit them here as ONE
// source of truth and retire the split GitHub CI_* registry. Real prod/dev
// values stay human-set on the Encore Platform ONLY and are NEVER committed.
//
// THESE ARE NOT SECRETS
// -------------------------------------------------------------------------
// Every value below is a deterministic, public placeholder (the literal
// string "of-zeroes" / "placeholder" appears in each). They are safe to
// commit, safe to print in CI logs, and identical for every developer and
// every CI run. Provisioning real credentials is a separate, human-only
// step on the Encore Platform (see apps/api/SECRETS.md).
//
// FORMAT
// -------------------------------------------------------------------------
// This file is valid CUE in the exact shape Encore expects for
// apps/api/.secrets.local.cue (top-level `<GoFieldName>: "<value>"`), so the
// CI workflow and the local provisioning script consume it WITHOUT any
// translation layer:
//   - Local dev: copy this file to apps/api/.secrets.local.cue
//     (or run apps/api/scripts/secrets-provision.sh).
//   - CI (apps-api-ci.yml): copies this file to apps/api/.secrets.local.cue
//     before `encore test`.
//   - Platform local/pr env types: apps/api/scripts/secrets-provision.sh
//     pipes each value into `encore secret set --type local,pr <Field>`.
//
// FIELD NAMES ARE THE CONTRACT
// -------------------------------------------------------------------------
// Each key below MUST match a field of `var secrets struct` in
// apps/api/auth/secrets.go verbatim (Go PascalCase). The CI drift-check
// (apps-api-ci.yml "secrets SoT drift-check" gate) FAILS the build if any
// secrets.go field is missing here — that catches "added the Go guard but
// forgot to provision" at PR time. See SPEC §3.5 for the
// logical-name ↔ Go-field mapping.
//
// ADD A SECRET: add the Go field + init guard in auth/secrets.go, then add a
// matching placeholder line here, then run secrets-provision.sh for local/pr.
// A human sets the real prod/dev value on the Encore Platform. Full
// procedure in apps/api/SECRETS.md.

MemoryDEK:               "nonprod-memory-dek-32-bytes-of-zeroes-placeholder"
APIKeyHMACSecret:        "nonprod-api-key-hmac-secret-32-bytes-of-zeroes-x"
GitHubOAuthClientID:     "nonprod-github-oauth-client-id-placeholder"
GitHubOAuthClientSecret: "nonprod-github-oauth-client-secret-placeholder"
GitHubOAuthRedirectURI:  "http://localhost:4321/auth/callback"
