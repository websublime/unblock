//! Shared versioning helpers for every `unblock-policy` decision contract (plan §2 `contract.rs`;
//! FR-12/FR-18 self-describing outputs).
//!
//! The [`Contract`] trait pins each contract output's stable `unblock.<name>.vN` schema string as a
//! `const`, and [`ContractEnvelope`] is the uniform wrapper that carries that `schema` (as the
//! **first** field, so a self-describing format like JSON emits it first), a `generated_at`
//! timestamp, and the typed payload — so scheduler/coordination/gate outputs all serialize with a
//! consistent shape. [`contract_versions`] lists every active schema string (sorted/stable) and
//! feeds `Capabilities.contract_version` via engine→mcp (spine §5.4).
//!
//! The envelope is introduced in v1 (so the ready/cache outputs can opt in) but is exercised from
//! v1.1 on, when the scheduler/coordination/gate contracts land.

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The root of the policy contract-version family (FR-12/FR-18).
///
/// A labelled seam: the per-contract schema strings (`unblock.scheduler.v1`,
/// `unblock.coordination.v1`, …) are introduced with their contracts in v1.1+; this root is used
/// from v1.1 on for the family-level capability reporting.
pub const POLICY_CONTRACT_VERSION: &str = "unblock.policy.v1";

/// A versioned decision contract: every contract output type pins its stable schema string here.
///
/// The string is the `unblock.<contract>.vN` identifier surfaced to clients (FR-12). A contract
/// bump (`.v1` → `.v2`) is additive — the old string is never removed (see the contract suite).
///
/// # Examples
///
/// ```
/// use unblock_policy::Contract;
///
/// struct Demo;
/// impl Contract for Demo {
///     const SCHEMA: &'static str = "unblock.demo.v1";
/// }
/// assert_eq!(Demo::SCHEMA, "unblock.demo.v1");
/// ```
pub trait Contract {
    /// The stable `unblock.<contract>.vN` schema identifier for this contract output.
    const SCHEMA: &'static str;
}

/// The uniform self-describing wrapper around a decision-contract payload (FR-12/FR-18).
///
/// `schema` is declared **first**, so a self-describing serializer (e.g. JSON) emits it as the
/// leading field — clients can branch on the schema before parsing the payload. `generated_at`
/// timestamps the decision (supplied by the caller — policy has no clock). The envelope round-trips
/// losslessly for any `T: Serialize + Deserialize`.
///
/// # Examples
///
/// ```
/// use unblock_policy::ContractEnvelope;
/// use chrono::{TimeZone, Utc};
///
/// let env = ContractEnvelope {
///     schema: "unblock.demo.v1".to_string(),
///     generated_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
///     payload: 42_u32,
/// };
/// let json = serde_json::to_string(&env).unwrap();
/// assert!(json.starts_with("{\"schema\":"));
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ContractEnvelope<T> {
    /// The contract schema string (`unblock.<contract>.vN`) — the **first** serialized field.
    pub schema: String,
    /// When this contract output was generated (caller-supplied; policy has no clock).
    pub generated_at: DateTime<Utc>,
    /// The typed contract payload.
    pub payload: T,
}

impl<T> ContractEnvelope<T> {
    /// Wrap a payload in an envelope carrying the [`Contract::SCHEMA`] of a contract type `C` and a
    /// caller-supplied `generated_at`.
    ///
    /// # Examples
    ///
    /// ```
    /// use unblock_policy::{Contract, ContractEnvelope};
    /// use chrono::{TimeZone, Utc};
    ///
    /// struct Demo;
    /// impl Contract for Demo { const SCHEMA: &'static str = "unblock.demo.v1"; }
    ///
    /// let now = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
    /// let env = ContractEnvelope::for_contract::<Demo>(now, "hello");
    /// assert_eq!(env.schema, "unblock.demo.v1");
    /// assert_eq!(env.payload, "hello");
    /// ```
    #[must_use]
    pub fn for_contract<C: Contract>(generated_at: DateTime<Utc>, payload: T) -> Self {
        Self {
            schema: C::SCHEMA.to_string(),
            generated_at,
            payload,
        }
    }
}

/// Every active contract schema string for this release, **sorted** and stable.
///
/// Feeds the engine→mcp `Capabilities` self-description (spine §5.4). In v1 the policy crate ships
/// no per-contract output yet (the scheduler/coordination/gate contracts land in v1.1), so the v1
/// list contains only [`POLICY_CONTRACT_VERSION`]. Later releases extend it **additively** (a
/// schema string is never removed — the contract suite pins this).
///
/// # Examples
///
/// ```
/// use unblock_policy::contract_versions;
///
/// let versions = contract_versions();
/// assert!(versions.contains(&"unblock.policy.v1"));
/// // Sorted + de-duplicated.
/// let mut sorted = versions.clone();
/// sorted.sort_unstable();
/// sorted.dedup();
/// assert_eq!(versions, sorted);
/// ```
#[must_use]
pub fn contract_versions() -> Vec<&'static str> {
    let mut versions = vec![POLICY_CONTRACT_VERSION];
    versions.sort_unstable();
    versions.dedup();
    versions
}

#[cfg(test)]
mod tests {
    use super::{Contract, ContractEnvelope, POLICY_CONTRACT_VERSION, contract_versions};
    use chrono::{TimeZone, Utc};

    struct Demo;
    impl Contract for Demo {
        const SCHEMA: &'static str = "unblock.demo.v1";
    }

    #[test]
    fn schema_serializes_first() {
        let env = ContractEnvelope {
            schema: "unblock.demo.v1".to_string(),
            generated_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            payload: 7_u32,
        };
        let json = serde_json::to_string(&env).unwrap();
        assert!(json.starts_with("{\"schema\":\"unblock.demo.v1\""));
    }

    #[test]
    fn envelope_round_trips() {
        let env = ContractEnvelope {
            schema: "unblock.demo.v1".to_string(),
            generated_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            payload: vec!["a".to_string(), "b".to_string()],
        };
        let json = serde_json::to_string(&env).unwrap();
        let back: ContractEnvelope<Vec<String>> = serde_json::from_str(&json).unwrap();
        assert_eq!(env, back);
    }

    #[test]
    fn for_contract_uses_schema_const() {
        let now = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let env = ContractEnvelope::for_contract::<Demo>(now, 1_u8);
        assert_eq!(env.schema, Demo::SCHEMA);
    }

    #[test]
    fn versions_are_sorted_stable_and_contain_root() {
        let versions = contract_versions();
        assert!(versions.contains(&POLICY_CONTRACT_VERSION));
        let mut sorted = versions.clone();
        sorted.sort_unstable();
        assert_eq!(versions, sorted);
        // Idempotent / stable across calls.
        assert_eq!(contract_versions(), contract_versions());
    }
}
