// Standalone entry point for the no_direct_is_ready_write analyzer.
// Runs as a single-analyzer driver via golang.org/x/tools/go/analysis
// /singlechecker. Two consumption paths:
//
//  1. Direct invocation:  go run ./shared/lint/cmd/no_direct_is_ready_write ./...
//  2. golangci-lint custom plugin: build the analyzer into a custom
//     golangci-lint binary via the module-plugin system; configure
//     `linters-settings.custom.no_direct_is_ready_write` in
//     apps/api/.golangci.yml.
//
// The singlechecker runtime adds the standard `-V`, `-flags`, and
// `-fix` switches expected by go/analysis tooling. No additional
// flags are added here.
package main

import (
	"golang.org/x/tools/go/analysis/singlechecker"

	"encore.app/shared/lint"
)

func main() {
	singlechecker.Main(lint.NoDirectIsReadyWriteAnalyzer)
}
