//! `RouterSettings` and its sub-settings (`app.freerouting.settings.*`).
//!
//! As in Java, every field is nullable (`Option`) so one type serves both as a
//! partial settings source and as the merged result; `None` means "this source
//! has no opinion". Nested sub-settings are always present (the Java
//! constructor creates them). Getters reproduce Java's fallbacks.

use crate::jutil::{java_equals_ignore_case, java_is_blank, java_round};

/// `RouterSettings.ALGORITHM_CURRENT`.
pub const ALGORITHM_CURRENT: &str = "freerouting-router";
/// `RouterSettings.ALGORITHM_V19`.
pub const ALGORITHM_V19: &str = "freerouting-router-v19";
pub const MIN_BEND_COST: f64 = 0.0;
pub const MAX_BEND_COST: f64 = 9.9;

/// Selects the router-board score formula (`RouterScoringVersion`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouterScoringVersion {
    V1Legacy,
    V2Continuous,
}

/// Selects the optimizer-board score formula (`OptimizerScoringVersion`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptimizerScoringVersion {
    V1Legacy,
    V2LowerBound,
}

/// `autoroute.BoardUpdateStrategy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoardUpdateStrategy {
    GlobalOptimal,
}

/// `autoroute.ItemSelectionStrategy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemSelectionStrategy {
    Sequential,
    Prioritized,
}

/// Java enum constant names, for reflection-style parsing (`Enum.name()`).
pub(crate) trait JavaEnum: Sized + Copy + 'static {
    const VALUES: &'static [(&'static str, Self)];
}

impl JavaEnum for RouterScoringVersion {
    const VALUES: &'static [(&'static str, Self)] = &[
        ("V1_LEGACY", Self::V1Legacy),
        ("V2_CONTINUOUS", Self::V2Continuous),
    ];
}
impl JavaEnum for OptimizerScoringVersion {
    const VALUES: &'static [(&'static str, Self)] = &[
        ("V1_LEGACY", Self::V1Legacy),
        ("V2_LOWER_BOUND", Self::V2LowerBound),
    ];
}
impl JavaEnum for BoardUpdateStrategy {
    const VALUES: &'static [(&'static str, Self)] = &[("GLOBAL_OPTIMAL", Self::GlobalOptimal)];
}
impl JavaEnum for ItemSelectionStrategy {
    const VALUES: &'static [(&'static str, Self)] = &[
        ("SEQUENTIAL", Self::Sequential),
        ("PRIORITIZED", Self::Prioritized),
    ];
}

/// Execution knobs for the batch autorouter stage (`AutorouterSettings`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AutorouterSettings {
    pub enabled: Option<bool>,
    pub algorithm: Option<String>,
    /// 0 means no limit.
    pub max_passes: Option<i32>,
    /// fastroute: the stagnation rules do not stop the autorouter before this many passes
    /// (0 = none; `max_passes` and `--max-time` still apply).
    pub min_passes: Option<i32>,
    pub max_items: Option<i32>,
    pub max_threads: Option<i32>,
    pub save_intermediate_stages: Option<bool>,
    pub ignore_net_classes: Option<Vec<String>>,
}

/// SMD fanout pre-pass settings (`FanoutSettings`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FanoutSettings {
    pub enabled: Option<bool>,
    pub max_passes: Option<i32>,
    pub max_items: Option<i32>,
    pub max_milliseconds_per_pin: Option<i64>,
    pub ripup_allowed: Option<bool>,
    pub min_escape_length_mm: Option<f64>,
    pub max_escape_length_mm: Option<f64>,
    pub start_via_diameter_mm: Option<f64>,
    pub end_via_diameter_mm: Option<f64>,
    /// "inner_first", "outer_first" or "unsorted".
    pub pin_sorting_order: Option<String>,
    pub fallback_to_board_vias: Option<bool>,
    /// Java `timeoutString`; `None` = no timeout.
    pub timeout_string: Option<String>,
}

/// Route optimizer settings (`OptimizerSettings`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OptimizerSettings {
    pub enabled: Option<bool>,
    pub algorithm: Option<String>,
    pub max_passes: Option<i32>,
    pub max_items: Option<i32>,
    pub max_threads: Option<i32>,
    /// Percentage (2.5 = 2.5 %). JSON/CLI name `improvement_threshold`.
    pub optimization_improvement_threshold: Option<f32>,
    pub enable_preflight_guards: Option<bool>,
    pub max_consecutive_failures: Option<i32>,
    pub max_consecutive_failures_pass1: Option<i32>,
    pub additional_ripup_cost_factor_at_start: Option<i32>,
    pub trace_ripup_cost_factor: Option<f32>,
    pub max_autoroute_passes: Option<i32>,
    pub board_update_strategy: Option<BoardUpdateStrategy>,
    pub item_selection_strategy: Option<ItemSelectionStrategy>,
    /// Java `timeoutString`; `None` = no timeout.
    pub timeout_string: Option<String>,
}

/// Search / rip-up costs (`RoutingCostSettings`, JSON name `scoring`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RoutingCostSettings {
    /// Per-layer cost of 1 mm in the preferred direction.
    pub preferred_direction_trace_cost: Option<Vec<f64>>,
    /// Per-layer cost of 1 mm against the preferred direction.
    pub undesired_direction_trace_cost: Option<Vec<f64>>,
    pub default_preferred_direction_trace_cost: Option<f64>,
    pub default_undesired_direction_trace_cost: Option<f64>,
    pub via_costs: Option<i32>,
    pub plane_via_costs: Option<i32>,
    pub start_ripup_costs: Option<i32>,
    pub default_bend_cost: Option<f64>,
    pub unrouted_net_penalty: Option<f32>,
    pub clearance_violation_penalty: Option<f32>,
    pub bend_penalty: Option<f32>,
}

/// Router-board score weights (`RouterScoreSettings`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RouterScoreSettings {
    pub version: Option<RouterScoringVersion>,
    pub unrouted_connection_weight: Option<f32>,
    pub unrouted_free_fraction: Option<f32>,
    pub unrouted_first_half_weight: Option<f32>,
    pub unrouted_second_half_weight: Option<f32>,
    pub clearance_violation_count_weight: Option<f32>,
    pub clearance_violation_depth_weight: Option<f32>,
    pub clearance_violation_depth_scale: Option<f32>,
}

/// Optimizer-board score weights (`OptimizerScoreSettings`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OptimizerScoreSettings {
    pub version: Option<OptimizerScoringVersion>,
    pub excess_wire_length_weight: Option<f32>,
    pub excess_via_weight: Option<f32>,
    pub excess_bend_weight: Option<f32>,
    pub length_floor: Option<f32>,
    pub difficulty_scale_floor: Option<f32>,
}

/// Settings for one board layer (`LayerSettings`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LayerSettings {
    pub routable: Option<bool>,
    pub preferred_direction_horizontal: Option<bool>,
    pub bend_cost: Option<f64>,
    pub preferred_direction_trace_cost: Option<f64>,
    pub undesired_direction_trace_cost: Option<f64>,
}

/// Mutable router configuration (`RouterSettings`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RouterSettings {
    pub autorouter: AutorouterSettings,
    pub fanout: FanoutSettings,
    pub copper_to_edge_clearance_um: Option<f64>,
    pub hole_clearance_um: Option<f64>,
    /// fastroute extension (not in Freerouting): the minimum drill-to-drill gap between two vias of the
    /// SAME net. When set, the clearance check judges a same-net via pair by this (the fab rule for
    /// same-net vias) instead of the copper clearance between different nets; routing is unchanged.
    pub same_net_via_hole_to_hole_um: Option<f64>,
    /// fastroute extension: areas where a net class's class-pair clearances do not apply, as KiCad's rule
    /// areas scope a rule ("not inside the USB pin field"). Entries "CLASS@x0:y0:x1:y1@base_um", the box in
    /// DSN units: a pair with an item of CLASS whose shape touches the box is held to base_um instead.
    /// Clearance check only; routing is unchanged.
    pub clearance_exempt_areas: Option<Vec<String>>,
    pub clearance_tolerance_um: Option<f64>,
    pub plane_nets: Option<Vec<String>>,
    pub plane_as_obstacle: Option<bool>,
    pub neck_width_um: Option<f64>,
    /// fastroute extension (not in Freerouting): narrowest trace width the
    /// router may create by neck-down; 0/unset = no limit (Java behaviour).
    pub min_trace_width_um: Option<f64>,
    pub strict_drc: Option<bool>,
    /// Java `jobTimeoutString` (JSON `job_timeout`), e.g. "12:00:00".
    pub job_timeout_string: Option<String>,
    pub layers: Option<Vec<LayerSettings>>,
    pub validation_warnings: Option<Vec<String>>,
    pub trace_pull_tight_accuracy: Option<i32>,
    /// JSON name `allowed_via_types`.
    pub vias_allowed: Option<bool>,
    pub automatic_neckdown: Option<bool>,
    pub optimizer: OptimizerSettings,
    /// Java field `scoring` (routing costs).
    pub scoring: RoutingCostSettings,
    pub router_scoring: RouterScoreSettings,
    pub optimizer_scoring: OptimizerScoreSettings,
    /// Legacy flat thread knob; not read by the headless pipeline.
    pub max_threads: Option<i32>,
    pub result_json_path: Option<String>,
    pub board_specific_trace_costs_applied: Option<bool>,
}

/// Horizontal / vertical trace cost of one layer
/// (`AutorouteControl.ExpansionCostFactor`, built by `getTraceCosts`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExpansionCostFactor {
    pub horizontal: f64,
    pub vertical: f64,
}

/// Board data needed by `applyBoardSpecificOptimizations`.
#[derive(Debug, Clone, Copy)]
pub struct BoardLayerInfo<'a> {
    /// `board.boundingBox.width()` (IntBox).
    pub bounding_box_width: i32,
    /// `board.boundingBox.height()` (IntBox).
    pub bounding_box_height: i32,
    /// `board.layerStructure.layers[i].isSignal`, one entry per board layer.
    pub layer_is_signal: &'a [bool],
}

// ---------------------------------------------------------------------------
// Merging (`ReflectionUtil.copyFields` / `applyNewValuesFrom`)

fn copy<T: Clone>(target: &mut Option<T>, source: &Option<T>, n: &mut usize) {
    if let Some(v) = source {
        *target = Some(v.clone());
        *n += 1;
    }
}

impl AutorouterSettings {
    fn copy_fields_from(&mut self, s: &Self) -> usize {
        let mut n = 0;
        copy(&mut self.enabled, &s.enabled, &mut n);
        copy(&mut self.algorithm, &s.algorithm, &mut n);
        copy(&mut self.max_passes, &s.max_passes, &mut n);
        copy(&mut self.min_passes, &s.min_passes, &mut n);
        copy(&mut self.max_items, &s.max_items, &mut n);
        copy(&mut self.max_threads, &s.max_threads, &mut n);
        copy(&mut self.save_intermediate_stages, &s.save_intermediate_stages, &mut n);
        copy(&mut self.ignore_net_classes, &s.ignore_net_classes, &mut n);
        n
    }
}

impl FanoutSettings {
    fn copy_fields_from(&mut self, s: &Self) -> usize {
        let mut n = 0;
        copy(&mut self.enabled, &s.enabled, &mut n);
        copy(&mut self.max_passes, &s.max_passes, &mut n);
        copy(&mut self.max_items, &s.max_items, &mut n);
        copy(&mut self.max_milliseconds_per_pin, &s.max_milliseconds_per_pin, &mut n);
        copy(&mut self.ripup_allowed, &s.ripup_allowed, &mut n);
        copy(&mut self.min_escape_length_mm, &s.min_escape_length_mm, &mut n);
        copy(&mut self.max_escape_length_mm, &s.max_escape_length_mm, &mut n);
        copy(&mut self.start_via_diameter_mm, &s.start_via_diameter_mm, &mut n);
        copy(&mut self.end_via_diameter_mm, &s.end_via_diameter_mm, &mut n);
        copy(&mut self.pin_sorting_order, &s.pin_sorting_order, &mut n);
        copy(&mut self.fallback_to_board_vias, &s.fallback_to_board_vias, &mut n);
        copy(&mut self.timeout_string, &s.timeout_string, &mut n);
        n
    }
}

impl OptimizerSettings {
    fn copy_fields_from(&mut self, s: &Self) -> usize {
        let mut n = 0;
        copy(&mut self.enabled, &s.enabled, &mut n);
        copy(&mut self.algorithm, &s.algorithm, &mut n);
        copy(&mut self.max_passes, &s.max_passes, &mut n);
        copy(&mut self.max_items, &s.max_items, &mut n);
        copy(&mut self.max_threads, &s.max_threads, &mut n);
        copy(
            &mut self.optimization_improvement_threshold,
            &s.optimization_improvement_threshold,
            &mut n,
        );
        copy(&mut self.enable_preflight_guards, &s.enable_preflight_guards, &mut n);
        copy(&mut self.max_consecutive_failures, &s.max_consecutive_failures, &mut n);
        copy(
            &mut self.max_consecutive_failures_pass1,
            &s.max_consecutive_failures_pass1,
            &mut n,
        );
        copy(
            &mut self.additional_ripup_cost_factor_at_start,
            &s.additional_ripup_cost_factor_at_start,
            &mut n,
        );
        copy(&mut self.trace_ripup_cost_factor, &s.trace_ripup_cost_factor, &mut n);
        copy(&mut self.max_autoroute_passes, &s.max_autoroute_passes, &mut n);
        copy(&mut self.board_update_strategy, &s.board_update_strategy, &mut n);
        copy(&mut self.item_selection_strategy, &s.item_selection_strategy, &mut n);
        copy(&mut self.timeout_string, &s.timeout_string, &mut n);
        n
    }
}

impl RoutingCostSettings {
    fn copy_fields_from(&mut self, s: &Self) -> usize {
        let mut n = 0;
        copy(
            &mut self.preferred_direction_trace_cost,
            &s.preferred_direction_trace_cost,
            &mut n,
        );
        copy(
            &mut self.undesired_direction_trace_cost,
            &s.undesired_direction_trace_cost,
            &mut n,
        );
        copy(
            &mut self.default_preferred_direction_trace_cost,
            &s.default_preferred_direction_trace_cost,
            &mut n,
        );
        copy(
            &mut self.default_undesired_direction_trace_cost,
            &s.default_undesired_direction_trace_cost,
            &mut n,
        );
        copy(&mut self.via_costs, &s.via_costs, &mut n);
        copy(&mut self.plane_via_costs, &s.plane_via_costs, &mut n);
        copy(&mut self.start_ripup_costs, &s.start_ripup_costs, &mut n);
        copy(&mut self.default_bend_cost, &s.default_bend_cost, &mut n);
        copy(&mut self.unrouted_net_penalty, &s.unrouted_net_penalty, &mut n);
        copy(&mut self.clearance_violation_penalty, &s.clearance_violation_penalty, &mut n);
        copy(&mut self.bend_penalty, &s.bend_penalty, &mut n);
        n
    }
}

impl RouterScoreSettings {
    fn copy_fields_from(&mut self, s: &Self) -> usize {
        let mut n = 0;
        copy(&mut self.version, &s.version, &mut n);
        copy(&mut self.unrouted_connection_weight, &s.unrouted_connection_weight, &mut n);
        copy(&mut self.unrouted_free_fraction, &s.unrouted_free_fraction, &mut n);
        copy(&mut self.unrouted_first_half_weight, &s.unrouted_first_half_weight, &mut n);
        copy(&mut self.unrouted_second_half_weight, &s.unrouted_second_half_weight, &mut n);
        copy(
            &mut self.clearance_violation_count_weight,
            &s.clearance_violation_count_weight,
            &mut n,
        );
        copy(
            &mut self.clearance_violation_depth_weight,
            &s.clearance_violation_depth_weight,
            &mut n,
        );
        copy(
            &mut self.clearance_violation_depth_scale,
            &s.clearance_violation_depth_scale,
            &mut n,
        );
        n
    }
}

impl OptimizerScoreSettings {
    fn copy_fields_from(&mut self, s: &Self) -> usize {
        let mut n = 0;
        copy(&mut self.version, &s.version, &mut n);
        copy(&mut self.excess_wire_length_weight, &s.excess_wire_length_weight, &mut n);
        copy(&mut self.excess_via_weight, &s.excess_via_weight, &mut n);
        copy(&mut self.excess_bend_weight, &s.excess_bend_weight, &mut n);
        copy(&mut self.length_floor, &s.length_floor, &mut n);
        copy(&mut self.difficulty_scale_floor, &s.difficulty_scale_floor, &mut n);
        n
    }
}

impl LayerSettings {
    fn copy_fields_from(&mut self, s: &Self) -> usize {
        let mut n = 0;
        copy(&mut self.routable, &s.routable, &mut n);
        copy(
            &mut self.preferred_direction_horizontal,
            &s.preferred_direction_horizontal,
            &mut n,
        );
        copy(&mut self.bend_cost, &s.bend_cost, &mut n);
        copy(
            &mut self.preferred_direction_trace_cost,
            &s.preferred_direction_trace_cost,
            &mut n,
        );
        copy(
            &mut self.undesired_direction_trace_cost,
            &s.undesired_direction_trace_cost,
            &mut n,
        );
        n
    }
}

impl RouterSettings {
    /// `new RouterSettings()`: all fields unset.
    pub fn new() -> Self {
        Self::default()
    }

    /// `applyNewValuesFrom` (`ReflectionUtil.copyFields`): copies every set
    /// field of `s` over `self`. Scalars, strings, enums, primitive/string
    /// arrays and lists are replaced; nested objects recurse; the `layers`
    /// array is merged element-wise when the target is at least as long,
    /// otherwise replaced by fresh elements holding only the source's set
    /// fields. Returns an approximate number of copied fields.
    pub fn apply_new_values_from(&mut self, s: &RouterSettings) -> usize {
        let mut n = self.autorouter.copy_fields_from(&s.autorouter);
        n += self.fanout.copy_fields_from(&s.fanout);
        copy(&mut self.copper_to_edge_clearance_um, &s.copper_to_edge_clearance_um, &mut n);
        copy(&mut self.hole_clearance_um, &s.hole_clearance_um, &mut n);
        copy(&mut self.same_net_via_hole_to_hole_um, &s.same_net_via_hole_to_hole_um, &mut n);
        copy(&mut self.clearance_exempt_areas, &s.clearance_exempt_areas, &mut n);
        copy(&mut self.clearance_tolerance_um, &s.clearance_tolerance_um, &mut n);
        copy(&mut self.plane_nets, &s.plane_nets, &mut n);
        copy(&mut self.plane_as_obstacle, &s.plane_as_obstacle, &mut n);
        copy(&mut self.neck_width_um, &s.neck_width_um, &mut n);
        copy(&mut self.min_trace_width_um, &s.min_trace_width_um, &mut n);
        copy(&mut self.strict_drc, &s.strict_drc, &mut n);
        copy(&mut self.job_timeout_string, &s.job_timeout_string, &mut n);
        if let Some(src) = &s.layers {
            let target_len = self.layers.as_ref().map_or(0, Vec::len);
            if target_len >= src.len() {
                let target = self.layers.as_mut().expect("non-empty or equal");
                for (t, sl) in target.iter_mut().zip(src) {
                    t.copy_fields_from(sl);
                }
            } else {
                self.layers = Some(
                    src.iter()
                        .map(|sl| {
                            let mut t = LayerSettings::default();
                            t.copy_fields_from(sl);
                            t
                        })
                        .collect(),
                );
            }
            n += src.len();
        }
        copy(&mut self.validation_warnings, &s.validation_warnings, &mut n);
        copy(&mut self.trace_pull_tight_accuracy, &s.trace_pull_tight_accuracy, &mut n);
        copy(&mut self.vias_allowed, &s.vias_allowed, &mut n);
        copy(&mut self.automatic_neckdown, &s.automatic_neckdown, &mut n);
        n += self.optimizer.copy_fields_from(&s.optimizer);
        n += self.scoring.copy_fields_from(&s.scoring);
        n += self.router_scoring.copy_fields_from(&s.router_scoring);
        n += self.optimizer_scoring.copy_fields_from(&s.optimizer_scoring);
        copy(&mut self.max_threads, &s.max_threads, &mut n);
        copy(&mut self.result_json_path, &s.result_json_path, &mut n);
        copy(
            &mut self.board_specific_trace_costs_applied,
            &s.board_specific_trace_costs_applied,
            &mut n,
        );
        n
    }

    /// `RouterSettings.clone()`. Note: Java's clone does not copy
    /// `resultJsonPath`; this reproduces that.
    pub fn java_clone(&self) -> Self {
        let mut r = self.clone();
        r.result_json_path = None;
        r
    }

    // -----------------------------------------------------------------------
    // Threads

    /// `defaultMaxThreads()`.
    pub fn default_max_threads(available_processors: i32) -> i32 {
        (available_processors - 1).max(1)
    }

    /// `normalizeMaxThreads(Integer)`.
    pub fn normalize_max_threads(value: Option<i32>, available_processors: i32) -> i32 {
        match value {
            None => Self::default_max_threads(available_processors),
            Some(v) if v < 0 => Self::default_max_threads(available_processors),
            Some(0) => available_processors,
            Some(v) => v.min(available_processors),
        }
    }

    /// `setMaxThreads`: normalizes and syncs the autorouter and optimizer knobs.
    pub fn set_max_threads(&mut self, value: Option<i32>, available_processors: i32) {
        let v = Self::normalize_max_threads(value, available_processors);
        self.max_threads = Some(v);
        self.autorouter.max_threads = Some(v);
        self.optimizer.max_threads = Some(v);
    }

    /// `getAutorouterMaxThreads`.
    pub fn get_autorouter_max_threads(&self, available_processors: i32) -> i32 {
        let configured = self.autorouter.max_threads.or(self.max_threads);
        Self::normalize_max_threads(configured, available_processors)
    }

    // -----------------------------------------------------------------------
    // Scalar getters / setters with Java fallbacks

    /// fastroute extension, see [`RouterSettings::min_trace_width_um`].
    pub fn get_min_trace_width_um(&self) -> f64 {
        match self.min_trace_width_um {
            Some(v) if v > 0.0 && v.is_finite() => v,
            _ => 0.0,
        }
    }

    pub fn get_neck_width_um(&self) -> f64 {
        match self.neck_width_um {
            Some(v) if v > 0.0 => v,
            _ => 0.0,
        }
    }

    pub fn is_strict_drc(&self) -> bool {
        self.strict_drc == Some(true)
    }

    pub fn get_start_ripup_costs(&self) -> i32 {
        self.scoring.start_ripup_costs.unwrap_or(1)
    }

    pub fn set_start_ripup_costs(&mut self, value: i32) {
        self.scoring.start_ripup_costs = Some(value.max(1));
    }

    pub fn get_run_router(&self) -> bool {
        self.autorouter.enabled.unwrap_or(true)
    }

    pub fn set_run_router(&mut self, value: bool) {
        self.autorouter.enabled = Some(value);
    }

    pub fn get_run_optimizer(&self) -> bool {
        self.optimizer.enabled.unwrap_or(false)
    }

    pub fn set_run_optimizer(&mut self, value: bool) {
        self.optimizer.enabled = Some(value);
    }

    /// `getRunFanout` (null -> true).
    pub fn get_run_fanout(&self) -> bool {
        self.fanout.enabled.unwrap_or(true)
    }

    /// `isFanoutEnabled` (null -> false).
    pub fn is_fanout_enabled(&self) -> bool {
        self.fanout.enabled == Some(true)
    }

    pub fn get_vias_allowed(&self) -> bool {
        self.vias_allowed.unwrap_or(true)
    }

    pub fn get_via_costs(&self) -> i32 {
        self.scoring.via_costs.unwrap_or(1)
    }

    pub fn set_via_costs(&mut self, value: i32) {
        self.scoring.via_costs = Some(value.max(1));
    }

    pub fn get_plane_via_costs(&self) -> i32 {
        self.scoring.plane_via_costs.unwrap_or(1)
    }

    pub fn set_plane_via_costs(&mut self, value: i32) {
        self.scoring.plane_via_costs = Some(value.max(1));
    }

    pub fn get_plane_nets(&self) -> Vec<String> {
        self.plane_nets.clone().unwrap_or_default()
    }

    pub fn is_plane_as_obstacle(&self) -> bool {
        self.plane_as_obstacle == Some(true)
    }

    pub fn get_automatic_neckdown(&self) -> bool {
        self.automatic_neckdown == Some(true)
    }

    pub fn are_board_specific_trace_costs_applied(&self) -> bool {
        self.board_specific_trace_costs_applied == Some(true)
    }

    // -----------------------------------------------------------------------
    // Layers

    pub fn get_layer_count(&self) -> usize {
        self.layers.as_ref().map_or(0, Vec::len)
    }

    fn layer(&self, layer: usize) -> Option<&LayerSettings> {
        self.layers.as_ref().and_then(|l| l.get(layer))
    }

    /// `setLayerCount`: (re)initializes the layer array and per-layer cost
    /// arrays (all 1.0). Note Java resets every layer even if the count is
    /// unchanged, but only clears `boardSpecificTraceCostsApplied` on a change.
    pub fn set_layer_count(&mut self, layer_count: usize) {
        if self.layers.as_ref().is_none_or(|l| l.len() != layer_count) {
            self.board_specific_trace_costs_applied = Some(false);
            self.layers = Some(vec![LayerSettings::default(); layer_count]);
        }
        let layers = self.layers.as_mut().expect("set above");
        for l in layers.iter_mut() {
            *l = LayerSettings {
                routable: Some(true),
                ..LayerSettings::default()
            };
        }
        self.scoring.preferred_direction_trace_cost = Some(vec![1.0; layer_count]);
        self.scoring.undesired_direction_trace_cost = Some(vec![1.0; layer_count]);
    }

    pub fn set_layer_active(&mut self, layer: usize, value: bool) {
        if layer >= self.get_layer_count() {
            return;
        }
        self.layers.as_mut().expect("in range")[layer].routable = Some(value);
    }

    pub fn get_layer_active(&self, layer: usize) -> bool {
        if layer >= self.get_layer_count() {
            return false;
        }
        self.layer(layer).and_then(|l| l.routable).unwrap_or(true)
    }

    pub fn set_bend_cost(&mut self, layer: usize, value: f64) {
        if layer >= self.get_layer_count() {
            return;
        }
        self.layers.as_mut().expect("in range")[layer].bend_cost =
            Some(MAX_BEND_COST.min(value).max(MIN_BEND_COST));
    }

    pub fn get_bend_cost(&self, layer: usize) -> f64 {
        if layer >= self.get_layer_count() {
            return 0.0;
        }
        match self.layer(layer).and_then(|l| l.bend_cost) {
            Some(v) => v,
            None => self
                .scoring
                .default_bend_cost
                .map_or(0.0, |d| MAX_BEND_COST.min(d).max(MIN_BEND_COST)),
        }
    }

    pub fn set_preferred_direction_is_horizontal(&mut self, layer: usize, value: bool) {
        if layer >= self.get_layer_count() {
            return;
        }
        self.layers.as_mut().expect("in range")[layer].preferred_direction_horizontal = Some(value);
    }

    /// Null falls back to `layer % 2 == 1`.
    pub fn get_preferred_direction_is_horizontal(&self, layer: usize) -> bool {
        if layer >= self.get_layer_count() {
            return false;
        }
        self.layer(layer)
            .and_then(|l| l.preferred_direction_horizontal)
            .unwrap_or(layer % 2 == 1)
    }

    /// `setPreferredDirectionTraceCosts`: clamps to >= 0.1 and marks costs as applied.
    pub fn set_preferred_direction_trace_costs(&mut self, layer: usize, value: f64) {
        let n = self.get_layer_count();
        if layer >= n {
            return;
        }
        let arr = self
            .scoring
            .preferred_direction_trace_cost
            .get_or_insert_with(Vec::new);
        if arr.len() != n {
            *arr = vec![0.0; n];
        }
        arr[layer] = value.max(0.1);
        let v = arr[layer];
        self.layers.as_mut().expect("in range")[layer].preferred_direction_trace_cost = Some(v);
        self.board_specific_trace_costs_applied = Some(true);
    }

    pub fn get_preferred_direction_trace_costs(&self, layer: usize) -> f64 {
        if layer >= self.get_layer_count() {
            return 0.0;
        }
        if let Some(v) = self.layer(layer).and_then(|l| l.preferred_direction_trace_cost) {
            return v;
        }
        match &self.scoring.preferred_direction_trace_cost {
            Some(a) if layer < a.len() => a[layer],
            _ => 1.0,
        }
    }

    /// `setAgainstPreferredDirectionTraceCosts`.
    pub fn set_against_preferred_direction_trace_costs(&mut self, layer: usize, value: f64) {
        let n = self.get_layer_count();
        if layer >= n {
            return;
        }
        let arr = self
            .scoring
            .undesired_direction_trace_cost
            .get_or_insert_with(Vec::new);
        if arr.len() != n {
            *arr = vec![0.0; n];
        }
        arr[layer] = value.max(0.1);
        let v = arr[layer];
        self.layers.as_mut().expect("in range")[layer].undesired_direction_trace_cost = Some(v);
        self.board_specific_trace_costs_applied = Some(true);
    }

    pub fn get_against_preferred_direction_trace_costs(&self, layer: usize) -> f64 {
        if layer >= self.get_layer_count() {
            return 0.0;
        }
        if let Some(v) = self.layer(layer).and_then(|l| l.undesired_direction_trace_cost) {
            return v;
        }
        match &self.scoring.undesired_direction_trace_cost {
            Some(a) if layer < a.len() => a[layer],
            _ => 1.0,
        }
    }

    pub fn get_horizontal_trace_costs(&self, layer: usize) -> f64 {
        if layer >= self.get_layer_count() {
            return 0.0;
        }
        if self.get_preferred_direction_is_horizontal(layer) {
            self.get_preferred_direction_trace_costs(layer)
        } else {
            self.get_against_preferred_direction_trace_costs(layer)
        }
    }

    pub fn get_vertical_trace_costs(&self, layer: usize) -> f64 {
        if layer >= self.get_layer_count() {
            return 0.0;
        }
        if self.get_preferred_direction_is_horizontal(layer) {
            self.get_against_preferred_direction_trace_costs(layer)
        } else {
            self.get_preferred_direction_trace_costs(layer)
        }
    }

    /// `getTraceCosts`: one factor per entry of `scoring.preferredDirectionTraceCost`.
    pub fn get_trace_costs(&self) -> Vec<ExpansionCostFactor> {
        let Some(arr) = &self.scoring.preferred_direction_trace_cost else {
            return Vec::new();
        };
        (0..arr.len())
            .map(|i| ExpansionCostFactor {
                horizontal: self.get_horizontal_trace_costs(i),
                vertical: self.get_vertical_trace_costs(i),
            })
            .collect()
    }

    /// `populateEffectiveLayerCosts`.
    pub fn populate_effective_layer_costs(&mut self) {
        let n = self.get_layer_count();
        let costs: Vec<(f64, f64)> = (0..n)
            .map(|i| {
                (
                    self.get_preferred_direction_trace_costs(i),
                    self.get_against_preferred_direction_trace_costs(i),
                )
            })
            .collect();
        if let Some(layers) = &mut self.layers {
            for (l, (p, u)) in layers.iter_mut().zip(costs) {
                l.preferred_direction_trace_cost = Some(p);
                l.undesired_direction_trace_cost = Some(u);
            }
        }
    }

    // -----------------------------------------------------------------------
    // Board-specific initialization

    /// `applyBoardSpecificOptimizationsIfNeeded`.
    pub fn apply_board_specific_optimizations_if_needed(&mut self, board: &BoardLayerInfo<'_>) {
        if self.get_layer_count() != board.layer_is_signal.len()
            || !self.are_board_specific_trace_costs_applied()
        {
            self.apply_board_specific_optimizations(board);
        }
    }

    /// `applyBoardSpecificOptimizations(RoutingBoard)`: sizes the layer and
    /// cost arrays, fills routability / bend cost / preferred direction
    /// defaults and (once) the per-layer trace costs from the board aspect
    /// ratio and signal-layer count. Logging is omitted.
    pub fn apply_board_specific_optimizations(&mut self, board: &BoardLayerInfo<'_>) {
        let horizontal_width = board.bounding_box_width as f64;
        let vertical_width = board.bounding_box_height as f64;
        let layer_count = board.layer_is_signal.len();

        let horizontal_add_costs_against_preferred_dir =
            0.1 * java_round(10.0 * horizontal_width / vertical_width) as f64;
        let vertical_add_costs_against_preferred_dir =
            0.1 * java_round(10.0 * vertical_width / horizontal_width) as f64;

        match &mut self.layers {
            Some(layers) if layers.len() == layer_count => {}
            _ => {
                self.board_specific_trace_costs_applied = Some(false);
                let old = self.layers.take().unwrap_or_default();
                let mut layers = old;
                layers.resize(layer_count, LayerSettings::default());
                self.layers = Some(layers);
            }
        }
        if self
            .scoring
            .preferred_direction_trace_cost
            .as_ref()
            .is_none_or(|a| a.len() != layer_count)
        {
            self.scoring.preferred_direction_trace_cost = Some(vec![0.0; layer_count]);
            self.board_specific_trace_costs_applied = Some(false);
        }
        if self
            .scoring
            .undesired_direction_trace_cost
            .as_ref()
            .is_none_or(|a| a.len() != layer_count)
        {
            self.scoring.undesired_direction_trace_cost = Some(vec![0.0; layer_count]);
            self.board_specific_trace_costs_applied = Some(false);
        }
        let default_pref = *self
            .scoring
            .default_preferred_direction_trace_cost
            .get_or_insert(1.0);
        let default_undesired = *self
            .scoring
            .default_undesired_direction_trace_cost
            .get_or_insert(1.0);
        let default_bend = self.scoring.default_bend_cost.unwrap_or(0.0);

        let mut current_preferred_direction_is_horizontal = horizontal_width < vertical_width;
        let initialize_trace_costs = !self.are_board_specific_trace_costs_applied();

        let layers = self.layers.as_mut().expect("sized above");
        let pref = self
            .scoring
            .preferred_direction_trace_cost
            .as_mut()
            .expect("sized above");
        let undesired = self
            .scoring
            .undesired_direction_trace_cost
            .as_mut()
            .expect("sized above");

        for i in 0..layer_count {
            let is_signal = board.layer_is_signal[i];
            if is_signal {
                current_preferred_direction_is_horizontal =
                    !current_preferred_direction_is_horizontal;
            }
            let l = &mut layers[i];
            if !is_signal {
                l.routable = Some(false);
            } else if l.routable.is_none() {
                l.routable = Some(true);
            }
            if l.bend_cost.is_none() {
                l.bend_cost = Some(default_bend);
            }
            if l.preferred_direction_horizontal.is_none() {
                l.preferred_direction_horizontal = Some(current_preferred_direction_is_horizontal);
            }
            if let Some(v) = l.preferred_direction_trace_cost {
                pref[i] = v;
            } else if initialize_trace_costs {
                pref[i] = default_pref;
            }
            if let Some(v) = l.undesired_direction_trace_cost {
                undesired[i] = v;
            } else if initialize_trace_costs {
                undesired[i] = default_undesired;
                if current_preferred_direction_is_horizontal {
                    undesired[i] += horizontal_add_costs_against_preferred_dir;
                } else {
                    undesired[i] += vertical_add_costs_against_preferred_dir;
                }
            }
        }
        if initialize_trace_costs {
            let signal_layer_count = board.layer_is_signal.iter().filter(|s| **s).count();
            if signal_layer_count > 2 {
                let outer_add_costs = 0.2 * signal_layer_count as f64;
                let last = layer_count - 1;
                if layers[0].preferred_direction_trace_cost.is_none() {
                    pref[0] += outer_add_costs;
                }
                if layers[last].preferred_direction_trace_cost.is_none() {
                    pref[last] += outer_add_costs;
                }
                if layers[0].undesired_direction_trace_cost.is_none() {
                    undesired[0] += outer_add_costs;
                }
                if layers[last].undesired_direction_trace_cost.is_none() {
                    undesired[last] += outer_add_costs;
                }
            }
            self.board_specific_trace_costs_applied = Some(true);
        }
    }

    /// `applyNetClassExclusions`: returns the indices of the board net classes
    /// (given by name, in board order) to mark `isIgnoredByAutorouter = true`.
    pub fn net_class_exclusions(&self, board_net_class_names: &[&str]) -> Vec<usize> {
        let Some(ignore) = &self.autorouter.ignore_net_classes else {
            return Vec::new();
        };
        let mut marked = vec![false; board_net_class_names.len()];
        for name in ignore {
            if java_is_blank(name) {
                continue;
            }
            for (i, class_name) in board_net_class_names.iter().enumerate() {
                if java_equals_ignore_case(class_name, name) {
                    marked[i] = true;
                }
            }
        }
        (0..marked.len()).filter(|&i| marked[i]).collect()
    }

    // -----------------------------------------------------------------------
    // Validation

    /// `validate()`.
    pub fn validate(&mut self, available_processors: i32) {
        self.validate_against_board(None, available_processors);
    }

    /// `validateAgainstBoard`: normalizes values and records warnings in
    /// `validation_warnings`. `board_net_class_names` replaces the board.
    pub fn validate_against_board(
        &mut self,
        board_net_class_names: Option<&[&str]>,
        available_processors: i32,
    ) {
        let mut warnings = Vec::new();
        if let Some(mp) = self.autorouter.max_passes {
            if mp < 0 || (mp > 9999 && mp != i32::MAX) {
                warnings.push(format!(
                    "Invalid maxPasses value: {mp}, using default 0 (no limit)"
                ));
                self.autorouter.max_passes = Some(0);
            }
        }
        let default_threads = Self::default_max_threads(available_processors);
        match self.max_threads {
            None => self.max_threads = Some(default_threads),
            Some(t) if t < 0 => {
                warnings.push(format!(
                    "Invalid maxThreads value: {t}, using {default_threads}"
                ));
                self.max_threads = Some(default_threads);
            }
            Some(t) if t > available_processors => {
                warnings.push(format!(
                    "Invalid maxThreads value: {t}, capping at {available_processors}"
                ));
                self.max_threads = Some(available_processors);
            }
            _ => {}
        }
        if let Some(a) = self.trace_pull_tight_accuracy {
            if a < 1 {
                warnings.push(format!(
                    "Invalid tracePullTightAccuracy value: {a}, using default 500"
                ));
                self.trace_pull_tight_accuracy = Some(500);
            }
        }
        if let Some(layers) = &mut self.layers {
            for (i, l) in layers.iter_mut().enumerate() {
                if let Some(b) = l.bend_cost {
                    // Explicit comparisons: NaN is not clamped, as in Java.
                    #[allow(clippy::manual_range_contains)]
                    let out_of_range = b < MIN_BEND_COST || b > MAX_BEND_COST;
                    if out_of_range {
                        let clamped = MAX_BEND_COST.min(b).max(MIN_BEND_COST);
                        warnings.push(format!(
                            "Layer {i} bendCost {b} out of range [{MIN_BEND_COST}, {MAX_BEND_COST}], clamped to {clamped}"
                        ));
                        l.bend_cost = Some(clamped);
                    }
                }
                if let Some(c) = l.preferred_direction_trace_cost {
                    if c < 0.1 {
                        warnings.push(format!(
                            "Layer {i} preferredDirectionTraceCost {c} below 0.1, clamped to 0.1"
                        ));
                        l.preferred_direction_trace_cost = Some(0.1);
                    }
                }
                if let Some(c) = l.undesired_direction_trace_cost {
                    if c < 0.1 {
                        warnings.push(format!(
                            "Layer {i} undesiredDirectionTraceCost {c} below 0.1, clamped to 0.1"
                        ));
                        l.undesired_direction_trace_cost = Some(0.1);
                    }
                }
            }
        }
        if let (Some(classes), Some(ignore)) =
            (board_net_class_names, &self.autorouter.ignore_net_classes)
        {
            for name in ignore {
                if java_is_blank(name) {
                    continue;
                }
                if !classes.iter().any(|c| java_equals_ignore_case(c, name)) {
                    warnings.push(format!(
                        "Unknown net class '{name}' specified in ignoreNetClasses (not present on board)"
                    ));
                }
            }
        }
        self.validation_warnings = Some(warnings);
    }
}
