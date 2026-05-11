// Wires the package-level `unblock` Database handle into the shared
// rbac builder so cross-service consumers (org, workitems via
// `apps/api/shared/rbac`) get a working query builder without
// re-importing the auth service.
//
// Why init() and not //encore:service: the auth package is already
// shaped as a function-only service (top-level //encore:api funcs +
// //encore:authhandler) so rewriting it as a `Service struct` to host
// an `initService` hook just to call rbac.Bind would touch every
// handler signature for no behavioural gain. Go's package-level init
// runs before any handler can execute and after package-level vars
// (db included) are constructed — the exact moment we need.
//
// Tracked risk (bead investigation): if a parallel test setup races
// with this init, behaviour is undefined. The risk is logged on bead
// unblock-tv8.34 and intentionally not mitigated here without a spec
// amendment (spec authors did not ask for a sync.Once or mutex).

package auth

import (
	"encore.app/shared/rbac"
)

// init binds the auth service's `unblock` database handle into the
// shared rbac builder. Encore guarantees package-level init runs
// before any //encore:api handler dispatch within the same service,
// and well before cross-service consumers (org, workitems) reach a
// rbac.For call site.
//
// dependency; the pattern is documented in this file's package
// comment and tracked on bead unblock-tv8.34.
//
//nolint:gochecknoinits // service-bootstrap wiring of a package-level
func init() {
	rbac.Bind(db)
}
