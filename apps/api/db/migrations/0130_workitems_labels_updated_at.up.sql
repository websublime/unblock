-- 0130_workitems_labels_updated_at.up.sql (round-16, bead unblock-tv8.75)
--
-- Adds the `updated_at` column to workitems.labels, closing the
-- contradiction between the SPEC §4.4 `Label.UpdatedAt` field (which has
-- always declared the column) and the original 0040_workitems.up.sql DDL
-- (which omitted it). Resolution DECIDED by Miguel 2026-06-11: the label
-- registry is mutable via Tool 22 (`update_label` rename/recolor) and
-- every other long-lived workitems row (items, milestones, comments)
-- already carries `updated_at`, so the column is ADDED rather than
-- dropping the struct field. The backing workitems.UpdateLabel RPC bumps
-- `updated_at` on every write (SPEC §4.4).
--
-- Up-only, pre-prod — no rows exist yet, so the NOT NULL DEFAULT now()
-- cannot fail on existing data. Additive to the §3.2 sequence; does not
-- renumber 0010..0120. Next free slot after the committed 0120 → 0130.

ALTER TABLE workitems.labels
    ADD COLUMN updated_at timestamptz NOT NULL DEFAULT now();
