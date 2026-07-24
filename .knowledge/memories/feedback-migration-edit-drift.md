---
name: feedback-migration-edit-drift
description: Editing an already-applied SQL migration in place silently drifts long-lived DBs (deployed Cloud + persistent local clusters); CI stays green because it runs fresh
type: gotcha
---

A version-numbered migration framework keys migrations by VERSION, not by content checksum. Editing a migration file that a database already applied does NOT re-run it — that DB keeps the pre-edit schema forever. Worked example: a CHECK constraint was widened in place (its allowed set grew) after some databases had already applied that migration, so those stale DBs kept the narrower constraint and inserts the widened rule should accept failed at runtime — while a fresh DB, applying the edited file from scratch, accepted them. The core write path was unaffected; only the rows the stale constraint rejected broke.

**Why this matters:** CI always starts a FRESH DB, so it applies the corrected migration and stays GREEN — the drift is invisible in CI and only bites long-lived environments (a deployed instance, a persistent local database). A green CI does NOT prove a deployed environment's schema matches the migration source.

**How to apply:** (1) Treat applied migrations as immutable. To change schema, add a NEW forward migration (idempotent — e.g. DROP ... IF EXISTS then re-create — so it is a no-op on fresh DBs and corrective on stale ones); never edit an applied one, even pre-production. (2) When something works in CI but fails at runtime, suspect schema drift: compare the LIVE schema against the migration source before blaming code. (3) Resetting a local database fixes it locally; a deployed environment needs the forward migration.
