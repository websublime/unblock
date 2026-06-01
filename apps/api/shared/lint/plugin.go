// plugin.go — golangci-lint v2 module-plugin registration for the two
// project-local analyzers in this package (NoDirectIsReadyWriteAnalyzer,
// NoRbacDynamicClauseAnalyzer).
//
// This file is the glue between the analyzers (whose authoritative
// declarations live in no_direct_is_ready_write.go and
// no_rbac_dynamic_clause.go) and the custom-gcl binary built by
// `golangci-lint custom` under apps/api/.custom-gcl.yml. Without this
// registration the analyzers compile but never load into the custom
// binary's linter table, and the corresponding entries under
// `linters.settings.custom.{no_direct_is_ready_write,no_rbac_dynamic_clause}`
// in apps/api/.golangci.yml resolve to "plugin not found".
//
// Wiring contract:
//
//   - Plugin name registered with register.Plugin MUST match the
//     `linters.settings.custom.<key>` key in apps/api/.golangci.yml,
//     and that key in turn drives the `linters.enable` entry name.
//   - LinterPlugin.BuildAnalyzers returns the analyzer instance(s)
//     exported by this package. We pass through the existing globals
//     verbatim — no per-invocation construction — so behaviour is
//     identical between the `go run ./shared/lint/cmd/<name>` direct
//     path and the custom-gcl path.
//   - LinterPlugin.GetLoadMode diverges by analyzer ON PURPOSE: each
//     plugin pins the lightest load mode it can actually run under, so
//     the two implementations below return different modes.
//     no_direct_is_ready_write pins register.LoadModeSyntax because it
//     inspects AST literals and import paths only (no type-info queries,
//     no SSA); forcing LoadModeTypesInfo there would make golangci-lint
//     populate go/types info that analyzer never consumes.
//     no_rbac_dynamic_clause pins register.LoadModeTypesInfo because it
//     reads pass.TypesInfo (receiver-type checks via .Selections,
//     package-qualified resolution via .Uses), and those maps are only
//     populated under the type-info load mode — syntax-only would leave
//     them nil and break the analyzer. The per-method GetLoadMode
//     comments below name the exact call sites each mode is dictated by.
//   - register.Plugin is called from init() so the registration fires
//     the moment the custom binary's main package blank-imports
//     encore.app/shared/lint via the module-plugin loader's generated
//     glue (the same pattern documented at
//     https://golangci-lint.run/docs/plugins/module-plugins).
//
// This package retains its existing `package lint` identity — the
// register-call lives alongside the analyzer source, not in a sibling
// package, because golangci-lint v2's module-plugin loader imports the
// module's named import path (`encore.app/shared/lint`) and expects the
// registration side-effect there.
//
// See:
//   - apps/api/.custom-gcl.yml (CI binary build recipe — pinned to
//     v2.7.2 per the orchestrator DECISION on unblock-tv8.6 dated
//     2026-05-26).
//   - apps/api/.golangci.yml lines 82-101 (linters.settings.custom
//     wiring; the names below match those keys verbatim).
//   - SPEC §11.2 NFR-10 (the gate set this plugin participates in).
//   - SPEC §11.3 (the two architectural invariants the analyzers
//     enforce).
package lint

import (
	"golang.org/x/tools/go/analysis"

	"github.com/golangci/plugin-module-register/register"
)

func init() {
	register.Plugin("no_direct_is_ready_write", newNoDirectIsReadyWritePlugin)
	register.Plugin("no_rbac_dynamic_clause", newNoRbacDynamicClausePlugin)
}

// noDirectIsReadyWritePlugin is the LinterPlugin wrapper around
// NoDirectIsReadyWriteAnalyzer. It carries no per-instance state — the
// analyzer is global and its configuration is hard-coded against SPEC
// §11.3's allow-list — so the `conf any` argument is intentionally
// ignored.
type noDirectIsReadyWritePlugin struct{}

func newNoDirectIsReadyWritePlugin(_ any) (register.LinterPlugin, error) {
	return &noDirectIsReadyWritePlugin{}, nil
}

// BuildAnalyzers returns the single analyzer this plugin contributes.
func (*noDirectIsReadyWritePlugin) BuildAnalyzers() ([]*analysis.Analyzer, error) {
	return []*analysis.Analyzer{NoDirectIsReadyWriteAnalyzer}, nil
}

// GetLoadMode pins syntax-only load. The analyzer inspects AST literals
// (no type info, no SSA).
func (*noDirectIsReadyWritePlugin) GetLoadMode() string {
	return register.LoadModeSyntax
}

// noRbacDynamicClausePlugin is the LinterPlugin wrapper around
// NoRbacDynamicClauseAnalyzer. Same shape and rationale as
// noDirectIsReadyWritePlugin above.
type noRbacDynamicClausePlugin struct{}

func newNoRbacDynamicClausePlugin(_ any) (register.LinterPlugin, error) {
	return &noRbacDynamicClausePlugin{}, nil
}

// BuildAnalyzers returns the single analyzer this plugin contributes.
func (*noRbacDynamicClausePlugin) BuildAnalyzers() ([]*analysis.Analyzer, error) {
	return []*analysis.Analyzer{NoRbacDynamicClauseAnalyzer}, nil
}

// GetLoadMode pins type-info load. The analyzer relies on
// pass.TypesInfo.Selections (Where receiver-type check) and
// pass.TypesInfo.Uses (For package-qualified resolution); both maps
// are only populated under LoadModeTypesInfo.
func (*noRbacDynamicClausePlugin) GetLoadMode() string {
	return register.LoadModeTypesInfo
}
