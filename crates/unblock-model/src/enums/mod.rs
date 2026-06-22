//! Open domain enums (spine §1.1–§1.5).
//!
//! `Status`, `IssueType`, and `DependencyType` are **open enums**: every known variant has a
//! stable wire string ([`Status::as_str`] etc.) and an unrecognised string deserializes into a
//! `Custom(String)` tail variant instead of erroring. They all **hand-roll** `Serialize` (via
//! `as_str`), `Deserialize` (unknown string → `Custom`), and `JsonSchema` (a plain string) — they
//! derive none of those and carry no `#[serde(...)]` attribute, because a `#[serde(untagged)]`
//! `Custom` would conflict with the hand-rolled `Deserialize` (spine §1.1, reconciled).
//!
//! There is one deliberate asymmetry, ported verbatim from the original: [`Status`] and
//! [`IssueType`] preserve the **original case** of a `Custom` value, while [`DependencyType`]
//! **lowercases** before storing it in `Custom`.
//!
//! [`Priority`] is a `#[serde(transparent)]` newtype over `i32` with numeric ordering, and
//! [`EventType`] is a closed-but-extensible enum with a hand-rolled string serde shape.

mod dependency;
mod event;
mod issue_type;
mod priority;
mod status;

pub use dependency::DependencyType;
pub use event::EventType;
pub use issue_type::IssueType;
pub use priority::Priority;
pub use status::Status;
