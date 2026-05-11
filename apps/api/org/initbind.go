// Wires the org service's `unblock` Database handle into the shared
// rbac builder. Mirrors apps/api/auth/initbind.go so the binding is
// service-local and removes any implicit ordering dependency on auth's
// init.
//
// Why a service-local Bind even though auth already binds: Encore
// guarantees per-service init() runs before any //encore:api dispatch,
// but cross-service initialisation order is not specified. If an org
// RPC handler dispatches before any auth handler executes — which can
// happen in unit-test paths that exercise org in isolation — rbac.Bind
// would not yet have run and rbac.For(...).Run() would return
// ErrNotBound. A second Bind from org closes that race; subsequent
// Bind calls overwrite the (identical) handle, which is documented as
// safe in shared/rbac/rbac.go's Bind contract.
//
// SPEC anchor: §10.1 (RBAC mechanism). Bead unblock-tv8.8 (B-2).

package org

import (
	"encore.app/shared/rbac"
)

// init binds the org service's `unblock` database handle into the
// shared rbac builder. See file header for rationale.
//
// dependency; the pattern mirrors apps/api/auth/initbind.go.
//
//nolint:gochecknoinits // service-bootstrap wiring of a package-level
func init() {
	rbac.Bind(db)
}
