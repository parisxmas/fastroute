//! `settings.sources.DefaultSettings`: hardcoded defaults (priority 0).

use crate::settings::{
    BoardUpdateStrategy, ItemSelectionStrategy, OptimizerScoringVersion, RouterScoringVersion,
    RouterSettings, ALGORITHM_CURRENT,
};

pub const DEFAULT_UNROUTED_NET_PENALTY: f32 = 5_000_000.0;
pub const DEFAULT_CLEARANCE_VIOLATION_PENALTY: f32 = 1_000_000.0;
pub const DEFAULT_BEND_PENALTY: f32 = 10.0;
pub const DEFAULT_VIA_COSTS: i32 = 50;
pub const DEFAULT_PLANE_VIA_COSTS: i32 = 5;
pub const DEFAULT_START_RIPUP_COSTS: i32 = 100;
pub const DEFAULT_PREFERRED_DIRECTION_TRACE_COST: f64 = 1.0;
pub const DEFAULT_UNDESIRED_DIRECTION_TRACE_COST: f64 = 1.0;
pub const DEFAULT_COPPER_TO_EDGE_CLEARANCE_UM: f64 = 250.0;
pub const DEFAULT_HOLE_CLEARANCE_UM: f64 = 0.0;
pub const DEFAULT_CLEARANCE_TOLERANCE_UM: f64 = 1.0;
pub const DEFAULT_NECK_WIDTH_UM: f64 = 0.0;
pub const DEFAULT_ROUTER_SCORING_VERSION: RouterScoringVersion = RouterScoringVersion::V2Continuous;
pub const DEFAULT_OPTIMIZER_SCORING_VERSION: OptimizerScoringVersion =
    OptimizerScoringVersion::V2LowerBound;
pub const DEFAULT_ROUTER_UNROUTED_CONNECTION_WEIGHT: f32 = 1000.0;
pub const DEFAULT_ROUTER_UNROUTED_FREE_FRACTION: f32 = 0.5;
/// `1000.0F / 3.0F` (float division).
pub const DEFAULT_ROUTER_UNROUTED_FIRST_HALF_WEIGHT: f32 = 1000.0f32 / 3.0f32;
/// `2000.0F / 3.0F` (float division).
pub const DEFAULT_ROUTER_UNROUTED_SECOND_HALF_WEIGHT: f32 = 2000.0f32 / 3.0f32;
pub const DEFAULT_ROUTER_CLEARANCE_COUNT_WEIGHT: f32 = 25.0;
pub const DEFAULT_ROUTER_CLEARANCE_DEPTH_WEIGHT: f32 = 300.0;
pub const DEFAULT_ROUTER_CLEARANCE_DEPTH_SCALE_UM: f32 = 1000.0;
pub const DEFAULT_OPTIMIZER_EXCESS_LENGTH_WEIGHT: f32 = 1000.0;
pub const DEFAULT_OPTIMIZER_EXCESS_VIA_WEIGHT: f32 = 2000.0;
pub const DEFAULT_OPTIMIZER_EXCESS_BEND_WEIGHT: f32 = 500.0;
pub const DEFAULT_OPTIMIZER_LENGTH_FLOOR: f32 = 1.0;
pub const DEFAULT_OPTIMIZER_DIFFICULTY_SCALE_FLOOR: f32 = 1.0;
pub const DEFAULT_OPTIMIZER_IMPROVEMENT_THRESHOLD: f32 = 2.5;

/// `DefaultSettings.getSettings()`. Thread counts use
/// `max(1, availableProcessors - 1)`. Layer arrays are left unset (they come
/// from the DSN source and `applyBoardSpecificOptimizations`).
pub fn default_settings(available_processors: i32) -> RouterSettings {
    let threads = (available_processors - 1).max(1);
    let mut s = RouterSettings::new();

    s.autorouter.enabled = Some(true);
    s.autorouter.algorithm = Some(ALGORITHM_CURRENT.to_string());
    s.job_timeout_string = Some("12:00:00".to_string());
    s.autorouter.max_passes = Some(0);
    s.autorouter.max_items = Some(i32::MAX);
    s.trace_pull_tight_accuracy = Some(500);
    s.vias_allowed = Some(true);
    s.automatic_neckdown = Some(true);
    s.autorouter.save_intermediate_stages = Some(false);
    s.autorouter.ignore_net_classes = Some(Vec::new());
    s.max_threads = Some(threads);
    s.autorouter.max_threads = Some(threads);
    s.copper_to_edge_clearance_um = Some(DEFAULT_COPPER_TO_EDGE_CLEARANCE_UM);
    s.hole_clearance_um = Some(DEFAULT_HOLE_CLEARANCE_UM);
    s.clearance_tolerance_um = Some(DEFAULT_CLEARANCE_TOLERANCE_UM);
    s.plane_nets = Some(Vec::new());
    s.plane_as_obstacle = Some(false);
    s.net_priority = Some(Vec::new());
    s.neck_width_um = Some(DEFAULT_NECK_WIDTH_UM);
    s.strict_drc = Some(false);

    s.fanout.enabled = Some(true);
    s.fanout.max_passes = Some(20);
    s.fanout.max_milliseconds_per_pin = Some(10000);
    s.fanout.ripup_allowed = Some(true);
    s.fanout.min_escape_length_mm = Some(2.5);
    s.fanout.max_escape_length_mm = Some(4.5);
    s.fanout.start_via_diameter_mm = Some(0.250);
    s.fanout.end_via_diameter_mm = Some(0.250);
    s.fanout.pin_sorting_order = Some("outer_first".to_string());
    s.fanout.max_items = Some(i32::MAX);
    s.fanout.fallback_to_board_vias = Some(true);

    s.optimizer.enabled = Some(true);
    s.optimizer.algorithm = Some("freerouting-optimizer".to_string());
    s.optimizer.max_passes = Some(100);
    s.optimizer.max_items = Some(i32::MAX);
    s.optimizer.max_threads = Some(threads);
    s.optimizer.optimization_improvement_threshold = Some(DEFAULT_OPTIMIZER_IMPROVEMENT_THRESHOLD);
    s.optimizer.board_update_strategy = Some(BoardUpdateStrategy::GlobalOptimal);
    s.optimizer.item_selection_strategy = Some(ItemSelectionStrategy::Sequential);
    s.optimizer.additional_ripup_cost_factor_at_start = Some(10);
    s.optimizer.trace_ripup_cost_factor = Some(0.6);
    s.optimizer.max_autoroute_passes = Some(6);
    s.optimizer.enable_preflight_guards = Some(true);
    s.optimizer.max_consecutive_failures = Some(50);
    s.optimizer.max_consecutive_failures_pass1 = Some(12);

    s.scoring.default_preferred_direction_trace_cost = Some(DEFAULT_PREFERRED_DIRECTION_TRACE_COST);
    s.scoring.default_undesired_direction_trace_cost = Some(DEFAULT_UNDESIRED_DIRECTION_TRACE_COST);
    s.scoring.via_costs = Some(DEFAULT_VIA_COSTS);
    s.scoring.plane_via_costs = Some(DEFAULT_PLANE_VIA_COSTS);
    s.scoring.start_ripup_costs = Some(DEFAULT_START_RIPUP_COSTS);
    s.scoring.unrouted_net_penalty = Some(DEFAULT_UNROUTED_NET_PENALTY);
    s.scoring.clearance_violation_penalty = Some(DEFAULT_CLEARANCE_VIOLATION_PENALTY);
    s.scoring.bend_penalty = Some(DEFAULT_BEND_PENALTY);
    s.scoring.default_bend_cost = Some(0.0);

    s.router_scoring.version = Some(DEFAULT_ROUTER_SCORING_VERSION);
    s.router_scoring.unrouted_connection_weight = Some(DEFAULT_ROUTER_UNROUTED_CONNECTION_WEIGHT);
    s.router_scoring.unrouted_free_fraction = Some(DEFAULT_ROUTER_UNROUTED_FREE_FRACTION);
    s.router_scoring.unrouted_first_half_weight = Some(DEFAULT_ROUTER_UNROUTED_FIRST_HALF_WEIGHT);
    s.router_scoring.unrouted_second_half_weight =
        Some(DEFAULT_ROUTER_UNROUTED_SECOND_HALF_WEIGHT);
    s.router_scoring.clearance_violation_count_weight = Some(DEFAULT_ROUTER_CLEARANCE_COUNT_WEIGHT);
    s.router_scoring.clearance_violation_depth_weight = Some(DEFAULT_ROUTER_CLEARANCE_DEPTH_WEIGHT);
    s.router_scoring.clearance_violation_depth_scale =
        Some(DEFAULT_ROUTER_CLEARANCE_DEPTH_SCALE_UM);

    s.optimizer_scoring.version = Some(DEFAULT_OPTIMIZER_SCORING_VERSION);
    s.optimizer_scoring.excess_wire_length_weight = Some(DEFAULT_OPTIMIZER_EXCESS_LENGTH_WEIGHT);
    s.optimizer_scoring.excess_via_weight = Some(DEFAULT_OPTIMIZER_EXCESS_VIA_WEIGHT);
    s.optimizer_scoring.excess_bend_weight = Some(DEFAULT_OPTIMIZER_EXCESS_BEND_WEIGHT);
    s.optimizer_scoring.length_floor = Some(DEFAULT_OPTIMIZER_LENGTH_FLOOR);
    s.optimizer_scoring.difficulty_scale_floor = Some(DEFAULT_OPTIMIZER_DIFFICULTY_SCALE_FLOOR);
    s
}

/// `Runtime.getRuntime().availableProcessors()` equivalent.
pub fn available_processors() -> i32 {
    std::thread::available_parallelism()
        .map(|n| i32::try_from(n.get()).unwrap_or(i32::MAX))
        .unwrap_or(1)
}
