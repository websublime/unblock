//! Property tests for the inheritance bookend selector (plan §3; NFR-16).
//!
//! - the output is never longer than two blocks;
//! - a tombstoned ancestor never appears in the output;
//! - selection is deterministic for a fixed chain + config;
//! - a disabled config yields no blocks.

use proptest::prelude::*;

use unblock_policy::proptest_support::{arb_ancestor_chain, arb_inheritance_config};
use unblock_policy::select_inherited_blocks;

proptest! {
    /// At most two bookends are ever selected.
    #[test]
    fn output_len_at_most_two(chain in arb_ancestor_chain(), cfg in arb_inheritance_config()) {
        prop_assert!(select_inherited_blocks(&chain, &cfg).len() <= 2);
    }

    /// No tombstoned ancestor is ever included.
    #[test]
    fn never_includes_tombstoned(chain in arb_ancestor_chain(), cfg in arb_inheritance_config()) {
        let blocks = select_inherited_blocks(&chain, &cfg);
        for block in &blocks {
            let source = chain.iter().find(|n| n.id == block.source_id);
            if let Some(node) = source {
                prop_assert!(!node.is_tombstone, "tombstoned ancestor leaked into output");
            }
        }
    }

    /// Selection is deterministic for a fixed chain + config.
    #[test]
    fn deterministic(chain in arb_ancestor_chain(), cfg in arb_inheritance_config()) {
        prop_assert_eq!(
            select_inherited_blocks(&chain, &cfg),
            select_inherited_blocks(&chain, &cfg)
        );
    }

    /// A disabled config never produces blocks regardless of the chain.
    #[test]
    fn disabled_config_yields_empty(chain in arb_ancestor_chain(), cfg in arb_inheritance_config()) {
        let disabled = unblock_policy::InheritanceConfig { enabled: false, fields: cfg.fields };
        prop_assert!(select_inherited_blocks(&chain, &disabled).is_empty());
    }
}
