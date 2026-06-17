-- C1 / Q7 (RESOLVED 2026-06-16 = DROP). Under the confirmed GitHub-App path
-- the webhook secret is app-level (one secret for all installs); HMAC
-- verifies against the Encore secret GITHUB_APP_WEBHOOK_SECRET, NOT a
-- per-install column. providers.installations is an empty stub
-- pre-production, so the drop is safe. Re-added by a future migration when
-- the OAuth-app / GitLab per-install fallback ships (v1.1). 0060 NOT edited.
-- Canonical DDL: docs/specs/02-spec-backend-complete.md §3.1(d) (bead unblock-8xb.7).
-- Forward-only: no .down.sql (the entire P01+P02 set is up-only — CG-13).
ALTER TABLE providers.installations DROP COLUMN webhook_secret_enc;
