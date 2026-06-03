# `apps/api` secrets runbook

Owner: Olive (infra). Bead `unblock-f6z`. Cross-references SPEC §3.5
(`docs/specs/01-spec-backend-mvp.md`).

This is the operational contract for the Encore backend's secrets. It explains
the one source of truth, the two registries that exist, how they map, and the
exact procedure to add a secret without recreating the deploy panic that
motivated this runbook.

---

## The five secrets

Declared once, in `apps/api/auth/secrets.go` (`var secrets struct`). The
`auth` service is the canonical consumer.

| Spec logical name           | Go field (manifest + CUE key) | Purpose                                                                 | Consumer                |
|-----------------------------|-------------------------------|-------------------------------------------------------------------------|-------------------------|
| `MEMORY_DEK`                | `MemoryDEK`                   | pgcrypto symmetric DEK for `*_enc` columns                              | `auth` (P01), all (P02) |
| `API_KEY_HMAC_SECRET`       | `APIKeyHMACSecret`            | `HMAC-SHA256` for MCP Bearer-key hashing + paginated cursor signing     | `auth`, `mcp`           |
| `GITHUB_OAUTH_CLIENT_ID`    | `GitHubOAuthClientID`         | OAuth2+PKCE client id                                                    | `auth.ExchangeOAuthCode`|
| `GITHUB_OAUTH_CLIENT_SECRET`| `GitHubOAuthClientSecret`     | OAuth2+PKCE client secret                                               | `auth.ExchangeOAuthCode`|
| `GITHUB_OAUTH_REDIRECT_URI` | `GitHubOAuthRedirectURI`      | OAuth2+PKCE registered callback (prevents `redirect_uri_mismatch`)      | `auth.ExchangeOAuthCode`|

Each Go field has a boot-time fail-fast guard (`auth.init()` for the four auth
secrets, `mcp/transport.go` for `APIKeyHMACSecret`). A missing value surfaces
as a startup panic at deploy — **never weaken these guards** (bead constraint).

---

## Name mapping: Encore ↔ GitHub ↔ CUE

There are two places a value can live. **Only one of them is a source of
truth, and the other no longer exists for non-prod.**

| Layer                        | Naming convention                  | Source of truth?                          |
|------------------------------|------------------------------------|-------------------------------------------|
| `var secrets struct` field   | Go PascalCase (`GitHubOAuthClientID`) | Defines the contract (what must exist) |
| `secrets.nonprod.cue` key    | Go PascalCase, verbatim            | ✅ **non-prod SoT** (committed)           |
| `.secrets.local.cue` key     | Go PascalCase, verbatim            | derived (copy of the SoT; gitignored)     |
| Encore Platform secret name  | Go PascalCase, verbatim            | ✅ **prod/dev SoT** (human-set, on platform)|
| GitHub Actions repo secret   | `CI_` + SCREAMING_SNAKE            | ❌ **RETIRED** (bead `unblock-f6z`)        |

The old GitHub `CI_*` registry (`CI_MEMORY_DEK`, etc.) was a second,
uncoordinated registry with a different naming convention and a
`${VAR:-default}` CI fallback that **masked** a missing secret — CI stayed
green while the platform deploy panicked. It is retired. CI now reads non-prod
values straight from the committed `secrets.nonprod.cue`.

### Boundary (be honest about what is enforced where)

- **CI drift-check** (`apps-api-ci.yml` "secrets SoT drift-check" gate)
  validates the Go secret structs ↔ `secrets.nonprod.cue` **only**. CI
  unlinks `encore.app` and has no platform auth, so it **cannot** see the
  Encore Platform secret matrix. The check is **bidirectional** — it enforces
  an exact bijection and hard-FAILs on either drift direction:
  - a declared struct field missing from the SoT (forgot to provision), and
  - a SoT key not declared in any struct (a stale/extra placeholder left
    behind after a Go secret was removed).

  The check scans **every `var secrets struct` in every (non-`_test.go`)
  package under `apps/api`** — today `auth/secrets.go`, `mcp/cursor.go`,
  `exitcriteriontest/secrets.go`, and `perftest/secrets.go` — discovering
  them dynamically (no hardcoded file list) and **unioning** their field
  names before comparing against the SoT. Per SPEC §3.5 the `auth` service is
  the canonical consumer and *should* declare the superset (the other three
  declare only the `APIKeyHMACSecret` subset today), but the check does not
  rely on that: it unions all packages so that a secret added to a non-`auth`
  struct **only** still must have a SoT placeholder. That closes the gap where
  a future non-`auth`-only secret would otherwise be invisible to both the SoT
  and the check. Both directions are a hard failure, never a warning — an
  ignorable check would re-open the same false-green gap that crashed the
  deploy.
- **Platform parity** (prod / dev / local / pr all populated) relies on:
  1. `scripts/secrets-provision.sh` for the `local` + `pr` placeholders,
  2. a human setting real `prod` + `dev` values on the platform,
  3. the deploy-time boot fail-fast as the last line of defence.

There is no automated check that the platform matrix is complete. The
fail-fast at deploy is the safety net; this runbook is the procedure.

---

## Source of truth: `secrets.nonprod.cue`

`apps/api/secrets.nonprod.cue` holds the **non-production placeholder values**
for every declared secret. These are deterministic, public, fake values
(`nonprod-…-placeholder`) — safe to commit and safe to print in logs. They are
valid CUE in the exact shape Encore expects, so they are consumed without any
translation:

- **Local dev**: `cp secrets.nonprod.cue .secrets.local.cue` (or run the
  provisioning script — see below). `.secrets.local.cue` is gitignored.
- **CI**: `apps-api-ci.yml` copies the SoT to `.secrets.local.cue` before
  `encore test`.
- **Platform `local` + `pr`**: `scripts/secrets-provision.sh` pipes each value
  into `encore secret set --type local,pr <Field>`.

**Real prod/dev values are NEVER committed.** They are set by a human on the
Encore Platform only.

---

## Provisioning script

`apps/api/scripts/secrets-provision.sh`:

```bash
./scripts/secrets-provision.sh
```

- Reads `secrets.nonprod.cue`, runs `encore secret set --type local,pr <Field>`
  for each entry (value piped via stdin).
- **Idempotent** — `encore secret set` overwrites, so re-running converges to
  the SoT.
- **Never touches `prod` or `dev`** — those carry real credentials.
- Requires the `encore` CLI authenticated (`encore auth login`) and the
  workspace linked (committed `apps/api/encore.app`).

---

## Procedure: add a new secret

Do all of these in one change so a Go guard never ships ahead of its value:

1. **Code**: add the field to `var secrets struct` in `apps/api/auth/secrets.go`
   and add a boot fail-fast guard in `auth.init()` (or the relevant service).
2. **SoT**: add a matching `FieldName: "nonprod-…-placeholder"` line to
   `apps/api/secrets.nonprod.cue`. (The CI drift-check fails the PR if you
   skip this.)
3. **Spec**: extend the SPEC §3.5 mapping table if the secret is contract-level.
4. **Local/pr platform**: run `./scripts/secrets-provision.sh`.
5. **prod/dev platform** (human, real value):
   ```bash
   encore secret set --type prod,dev <FieldName>
   # enter the real value at the prompt
   ```
6. **Update this runbook's five-secrets table.**

The CI drift-check guarantees step 2 cannot be forgotten silently; steps 4–5
are the platform side it cannot see — the deploy-time panic is the backstop.

---

## Orphan GitHub repo-secret cleanup (handoff to Miguel)

The retired `CI_*` registry plus a stale test token must be deleted from the
GitHub repo secrets. **Run these AFTER the workflow change in this bead has
merged to `main`** (so no in-flight run references them). Miguel executes —
the implementing agent does NOT run these.

Repo: `websublime/unblock`.

```bash
gh secret delete CI_MEMORY_DEK              --repo websublime/unblock
gh secret delete CI_API_KEY_HMAC_SECRET     --repo websublime/unblock
gh secret delete CI_GITHUB_OAUTH_CLIENT_ID  --repo websublime/unblock
gh secret delete CI_GITHUB_OAUTH_CLIENT_SECRET --repo websublime/unblock
gh secret delete UNBLOCK_TEST_TOKEN         --repo websublime/unblock
```

Notes:
- `CI_GITHUB_OAUTH_REDIRECT_URI` was never created, so there is nothing to
  delete for it.
- `UNBLOCK_TEST_TOKEN` has zero references anywhere in the repo (verified via
  grep) — stale.
- After deletion, `gh secret list --repo websublime/unblock` should show no
  `CI_*` entries and no `UNBLOCK_TEST_TOKEN`.
