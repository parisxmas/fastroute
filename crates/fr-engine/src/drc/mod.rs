//! Design rule checks needed by the routing pipeline and the scoring (porting unit U6, first
//! half).
//!
//! | Module | Java |
//! |---|---|
//! | [`clearance_violation`] | `drc/ClearanceViolation`, `Item.clearanceViolations()` + `Via.clearanceViolations()` |
//! | [`net_incompletes`] | `drc/NetIncompletes`, `drc/AirLine` |
//! | [`checker`] | `drc/DesignRulesChecker` (clearance violations, incompletes), `drc/UnconnectedItems` |
//!
//! Not ported: KiCad DRC report/JSON, `DrcSummaryResponse`, zone islands (API/GUI only).
//!
//! Parity: clearance violations (order, fields, dedupe) and all incomplete *counts* are exact;
//! the airlines of a net may differ from a given Java run where Java's result depends on identity
//! hash codes (see [`net_incompletes`]).
//!
//! Cost (for memoization in the pipeline): [`all_clearance_violations`] runs one clearance tree
//! query per tile shape of every item (plus a 16-step bisection per violating pair) — the most
//! expensive call, O(items × neighbours). [`DesignRulesChecker::calculate_all_incompletes`]
//! computes a connected set per group and a Delaunay triangulation per net with ≥ 2 groups;
//! `is_tail`/`normal_contacts` of every connectable item dominate. Both are pure functions of
//! the board state.

pub mod checker;
pub mod clearance_violation;
pub mod net_incompletes;
pub mod exact;

pub use checker::{all_clearance_violations, DesignRulesChecker, UnconnectedItems};
pub use clearance_violation::{
    aggregate_sorted_by_severity, clearance_violation_count, clearance_violations, clearance_violations_updating, smallest_clearance,
    Category, ClearanceViolation,
};
pub use net_incompletes::{AirLine, NetIncompletes};
