//! Port of `ReflectionUtil.setFieldValue` restricted to the `RouterSettings`
//! tree: dotted property paths used by CLI arguments and environment
//! variables (`--router.optimizer.max_threads=4`, `layers.routable=a,b`).
//!
//! Field lookup follows `getFieldByNameOrSerializedName`: a path segment
//! matches a field when, after removing underscores, it equals the Java field
//! name, the `@SerializedName` value or one of its alternates, ignoring case.
//! The first matching field in Java declaration order wins.

use crate::jutil::{
    java_parse_double, java_parse_float, java_parse_int, java_parse_long, java_split, java_trim,
    reflection_parse_bool,
};
use crate::settings::{
    AutorouterSettings, FanoutSettings, JavaEnum, LayerSettings, OptimizerScoreSettings,
    OptimizerSettings, RouterScoreSettings, RouterSettings, RoutingCostSettings,
};

/// A failed property assignment (Java: `NoSuchFieldException` or a conversion error).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetFieldError {
    NoSuchField(String),
    InvalidValue { field: String, value: String },
}

impl std::fmt::Display for SetFieldError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SetFieldError::NoSuchField(n) => {
                write!(f, "No field found with name or SerializedName: {n}")
            }
            SetFieldError::InvalidValue { field, value } => {
                write!(f, "cannot assign '{value}' to field '{field}'")
            }
        }
    }
}

impl std::error::Error for SetFieldError {}

type R = Result<(), SetFieldError>;

/// (Java field name, serialized name and alternates).
type FieldDesc = (&'static str, &'static [&'static str]);

fn norm(s: &str) -> String {
    s.chars().filter(|c| *c != '_').collect::<String>().to_lowercase()
}

fn find_field(fields: &[FieldDesc], name: &str) -> Result<&'static str, SetFieldError> {
    let n = norm(name);
    for (java, names) in fields {
        if names.iter().any(|a| norm(a) == n) || norm(java) == n {
            return Ok(java);
        }
    }
    Err(SetFieldError::NoSuchField(name.to_string()))
}

fn bad(field: &str, value: &str) -> SetFieldError {
    SetFieldError::InvalidValue {
        field: field.to_string(),
        value: value.to_string(),
    }
}

fn leaf(path: &[&str], field: &str) -> Result<(), SetFieldError> {
    // A path continuing below a scalar field cannot be resolved.
    if path.len() > 1 {
        Err(SetFieldError::NoSuchField(format!("{field}.{}", path[1])))
    } else {
        Ok(())
    }
}

fn set_i32(t: &mut Option<i32>, f: &str, v: &str) -> R {
    *t = Some(java_parse_int(v).ok_or_else(|| bad(f, v))?);
    Ok(())
}
fn set_i64(t: &mut Option<i64>, f: &str, v: &str) -> R {
    *t = Some(java_parse_long(v).ok_or_else(|| bad(f, v))?);
    Ok(())
}
fn set_f64(t: &mut Option<f64>, f: &str, v: &str) -> R {
    *t = Some(java_parse_double(v).ok_or_else(|| bad(f, v))?);
    Ok(())
}
fn set_f32(t: &mut Option<f32>, f: &str, v: &str) -> R {
    *t = Some(java_parse_float(v).ok_or_else(|| bad(f, v))?);
    Ok(())
}
fn set_bool(t: &mut Option<bool>, v: &str) -> R {
    *t = Some(reflection_parse_bool(v));
    Ok(())
}
fn set_string(t: &mut Option<String>, v: &str) -> R {
    *t = Some(v.to_string());
    Ok(())
}
fn set_enum<E: JavaEnum>(t: &mut Option<E>, f: &str, v: &str) -> R {
    let key = java_trim(v);
    let e = E::VALUES
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(key))
        .map(|(_, e)| *e)
        .ok_or_else(|| bad(f, v))?;
    *t = Some(e);
    Ok(())
}
/// `String[]` conversion: comma separated, trimmed, empty -> [].
fn set_strings(t: &mut Option<Vec<String>>, v: &str) -> R {
    let raw = java_trim(v);
    *t = Some(if raw.is_empty() {
        Vec::new()
    } else {
        java_split(raw, |c| c == ',')
            .into_iter()
            .map(|s| java_trim(s).to_string())
            .collect()
    });
    Ok(())
}
/// `double[]` conversion.
fn set_doubles(t: &mut Option<Vec<f64>>, f: &str, v: &str) -> R {
    let raw = java_trim(v);
    *t = Some(if raw.is_empty() {
        Vec::new()
    } else {
        java_split(raw, |c| c == ',')
            .into_iter()
            .map(|s| java_parse_double(java_trim(s)).ok_or_else(|| bad(f, v)))
            .collect::<Result<_, _>>()?
    });
    Ok(())
}

/// `ReflectionUtil.setFieldValue(settings, propertyName, value)`; the path is
/// split on `.`, `:` and `-`.
pub fn set_field_value(settings: &mut RouterSettings, property_name: &str, value: &str) -> R {
    let path = java_split(property_name, |c| matches!(c, '.' | ':' | '-'));
    if path.is_empty() {
        return Err(SetFieldError::NoSuchField(property_name.to_string()));
    }
    set_router(settings, &path, value)
}

const ROUTER_FIELDS: &[FieldDesc] = &[
    ("autorouter", &["autorouter"]),
    ("fanout", &["fanout"]),
    ("copperToEdgeClearanceUm", &["copper_to_edge_clearance_um"]),
    ("holeClearanceUm", &["hole_clearance_um"]),
    ("sameNetViaHoleToHoleUm", &["same_net_via_hole_to_hole_um"]),
    ("clearanceExemptAreas", &["clearance_exempt_areas"]),
    ("clearanceToleranceUm", &["clearance_tolerance_um"]),
    ("planeNets", &["plane_nets"]),
    (
        "planeAsObstacle",
        &[
            "plane_as_obstacle",
            "conduction_is_obstacle",
            "planeAsObstacle",
            "conductionIsObstacle",
        ],
    ),
    ("neckWidthUm", &["neck_width_um"]),
    ("minTraceWidthUm", &["min_trace_width_um"]),
    ("strictDrc", &["strict_drc"]),
    ("jobTimeoutString", &["job_timeout"]),
    ("layers", &["layers"]),
    ("validationWarnings", &["validation_warnings"]),
    (
        "tracePullTightAccuracy",
        &["trace_pull_tight_accuracy", "tracePullTightAccuracy"],
    ),
    ("viasAllowed", &["allowed_via_types"]),
    ("automaticNeckdown", &["automatic_neckdown", "automaticNeckdown"]),
    ("optimizer", &["optimizer"]),
    ("scoring", &["scoring"]),
    ("routerScoring", &["router_scoring"]),
    ("optimizerScoring", &["optimizer_scoring"]),
    ("maxThreads", &["max_threads"]),
    ("resultJsonPath", &["result_json"]),
    ("boardSpecificTraceCostsApplied", &[]),
];

fn set_router(s: &mut RouterSettings, path: &[&str], v: &str) -> R {
    let f = find_field(ROUTER_FIELDS, path[0])?;
    let rest = &path[1..];
    match f {
        "autorouter" => return nested(rest, f, |p| set_autorouter(&mut s.autorouter, p, v)),
        "fanout" => return nested(rest, f, |p| set_fanout(&mut s.fanout, p, v)),
        "optimizer" => return nested(rest, f, |p| set_optimizer(&mut s.optimizer, p, v)),
        "scoring" => return nested(rest, f, |p| set_scoring(&mut s.scoring, p, v)),
        "routerScoring" => {
            return nested(rest, f, |p| set_router_scoring(&mut s.router_scoring, p, v))
        }
        "optimizerScoring" => {
            return nested(rest, f, |p| set_optimizer_scoring(&mut s.optimizer_scoring, p, v))
        }
        "layers" => {
            if rest.is_empty() {
                return Err(bad(f, v));
            }
            // Array navigation: one comma-separated token per element.
            let tokens = java_split(v, |c| c == ',');
            let layers = s
                .layers
                .get_or_insert_with(|| vec![LayerSettings::default(); tokens.len()]);
            let limit = layers.len().min(tokens.len());
            for i in 0..limit {
                set_layer(&mut layers[i], rest, java_trim(tokens[i]))?;
            }
            return Ok(());
        }
        _ => {}
    }
    leaf(path, f)?;
    match f {
        "copperToEdgeClearanceUm" => set_f64(&mut s.copper_to_edge_clearance_um, f, v),
        "holeClearanceUm" => set_f64(&mut s.hole_clearance_um, f, v),
        "sameNetViaHoleToHoleUm" => set_f64(&mut s.same_net_via_hole_to_hole_um, f, v),
        "clearanceExemptAreas" => set_strings(&mut s.clearance_exempt_areas, v),
        "clearanceToleranceUm" => set_f64(&mut s.clearance_tolerance_um, f, v),
        "planeNets" => set_strings(&mut s.plane_nets, v),
        "planeAsObstacle" => set_bool(&mut s.plane_as_obstacle, v),
        "neckWidthUm" => set_f64(&mut s.neck_width_um, f, v),
        "minTraceWidthUm" => set_f64(&mut s.min_trace_width_um, f, v),
        "strictDrc" => set_bool(&mut s.strict_drc, v),
        "jobTimeoutString" => set_string(&mut s.job_timeout_string, v),
        "tracePullTightAccuracy" => set_i32(&mut s.trace_pull_tight_accuracy, f, v),
        "viasAllowed" => set_bool(&mut s.vias_allowed, v),
        "automaticNeckdown" => set_bool(&mut s.automatic_neckdown, v),
        "maxThreads" => set_i32(&mut s.max_threads, f, v),
        "resultJsonPath" => set_string(&mut s.result_json_path, v),
        "boardSpecificTraceCostsApplied" => set_bool(&mut s.board_specific_trace_costs_applied, v),
        // validationWarnings (a List) cannot be assigned from a string.
        _ => Err(bad(f, v)),
    }
}

fn nested(rest: &[&str], field: &str, f: impl FnOnce(&[&str]) -> R) -> R {
    if rest.is_empty() {
        // Assigning a string to an object-typed field fails in Java.
        Err(SetFieldError::InvalidValue {
            field: field.to_string(),
            value: String::new(),
        })
    } else {
        f(rest)
    }
}

const AUTOROUTER_FIELDS: &[FieldDesc] = &[
    ("enabled", &["enabled"]),
    ("algorithm", &["algorithm"]),
    ("maxPasses", &["max_passes"]),
    ("minPasses", &["min_passes"]),
    ("maxItems", &["max_items"]),
    ("maxThreads", &["max_threads"]),
    ("saveIntermediateStages", &["save_intermediate_stages"]),
    ("ignoreNetClasses", &["ignore_net_classes"]),
];

fn set_autorouter(s: &mut AutorouterSettings, path: &[&str], v: &str) -> R {
    let f = find_field(AUTOROUTER_FIELDS, path[0])?;
    leaf(path, f)?;
    match f {
        "enabled" => set_bool(&mut s.enabled, v),
        "algorithm" => set_string(&mut s.algorithm, v),
        "maxPasses" => set_i32(&mut s.max_passes, f, v),
        "minPasses" => set_i32(&mut s.min_passes, f, v),
        "maxItems" => set_i32(&mut s.max_items, f, v),
        "maxThreads" => set_i32(&mut s.max_threads, f, v),
        "saveIntermediateStages" => set_bool(&mut s.save_intermediate_stages, v),
        _ => set_strings(&mut s.ignore_net_classes, v),
    }
}

const FANOUT_FIELDS: &[FieldDesc] = &[
    ("enabled", &["enabled"]),
    ("maxPasses", &["max_passes"]),
    ("maxItems", &["max_items"]),
    ("maxMillisecondsPerPin", &["max_milliseconds_per_pin"]),
    ("ripupAllowed", &["ripup_allowed", "ripupAllowed"]),
    ("minEscapeLengthMm", &["min_escape_length_mm"]),
    ("maxEscapeLengthMm", &["max_escape_length_mm"]),
    ("startViaDiameterMm", &["start_via_diameter_mm"]),
    ("endViaDiameterMm", &["end_via_diameter_mm"]),
    ("pinSortingOrder", &["pin_sorting_order"]),
    ("fallbackToBoardVias", &["fallback_to_board_vias"]),
    ("timeoutString", &["timeout"]),
];

fn set_fanout(s: &mut FanoutSettings, path: &[&str], v: &str) -> R {
    let f = find_field(FANOUT_FIELDS, path[0])?;
    leaf(path, f)?;
    match f {
        "enabled" => set_bool(&mut s.enabled, v),
        "maxPasses" => set_i32(&mut s.max_passes, f, v),
        "maxItems" => set_i32(&mut s.max_items, f, v),
        "maxMillisecondsPerPin" => set_i64(&mut s.max_milliseconds_per_pin, f, v),
        "ripupAllowed" => set_bool(&mut s.ripup_allowed, v),
        "minEscapeLengthMm" => set_f64(&mut s.min_escape_length_mm, f, v),
        "maxEscapeLengthMm" => set_f64(&mut s.max_escape_length_mm, f, v),
        "startViaDiameterMm" => set_f64(&mut s.start_via_diameter_mm, f, v),
        "endViaDiameterMm" => set_f64(&mut s.end_via_diameter_mm, f, v),
        "pinSortingOrder" => set_string(&mut s.pin_sorting_order, v),
        "fallbackToBoardVias" => set_bool(&mut s.fallback_to_board_vias, v),
        _ => set_string(&mut s.timeout_string, v),
    }
}

const OPTIMIZER_FIELDS: &[FieldDesc] = &[
    ("enabled", &["enabled"]),
    ("algorithm", &["algorithm"]),
    ("maxPasses", &["max_passes"]),
    ("maxItems", &["max_items"]),
    ("maxThreads", &["max_threads"]),
    ("optimizationImprovementThreshold", &["improvement_threshold"]),
    ("enablePreflightGuards", &["enable_preflight_guards"]),
    ("maxConsecutiveFailures", &["max_consecutive_failures"]),
    ("maxConsecutiveFailuresPass1", &["max_consecutive_failures_pass1"]),
    (
        "additionalRipupCostFactorAtStart",
        &["additional_ripup_cost_factor_at_start"],
    ),
    ("traceRipupCostFactor", &["trace_ripup_cost_factor"]),
    ("maxAutoroutePasses", &["max_autoroute_passes"]),
    ("boardUpdateStrategy", &["board_update_strategy"]),
    ("itemSelectionStrategy", &["item_selection_strategy"]),
    ("timeoutString", &["timeout"]),
];

fn set_optimizer(s: &mut OptimizerSettings, path: &[&str], v: &str) -> R {
    let f = find_field(OPTIMIZER_FIELDS, path[0])?;
    leaf(path, f)?;
    match f {
        "enabled" => set_bool(&mut s.enabled, v),
        "algorithm" => set_string(&mut s.algorithm, v),
        "maxPasses" => set_i32(&mut s.max_passes, f, v),
        "maxItems" => set_i32(&mut s.max_items, f, v),
        "maxThreads" => set_i32(&mut s.max_threads, f, v),
        "optimizationImprovementThreshold" => {
            set_f32(&mut s.optimization_improvement_threshold, f, v)
        }
        "enablePreflightGuards" => set_bool(&mut s.enable_preflight_guards, v),
        "maxConsecutiveFailures" => set_i32(&mut s.max_consecutive_failures, f, v),
        "maxConsecutiveFailuresPass1" => set_i32(&mut s.max_consecutive_failures_pass1, f, v),
        "additionalRipupCostFactorAtStart" => {
            set_i32(&mut s.additional_ripup_cost_factor_at_start, f, v)
        }
        "traceRipupCostFactor" => set_f32(&mut s.trace_ripup_cost_factor, f, v),
        "maxAutoroutePasses" => set_i32(&mut s.max_autoroute_passes, f, v),
        "boardUpdateStrategy" => set_enum(&mut s.board_update_strategy, f, v),
        "itemSelectionStrategy" => set_enum(&mut s.item_selection_strategy, f, v),
        _ => set_string(&mut s.timeout_string, v),
    }
}

const SCORING_FIELDS: &[FieldDesc] = &[
    ("preferredDirectionTraceCost", &["preferred_direction_trace_cost"]),
    ("undesiredDirectionTraceCost", &["undesired_direction_trace_cost"]),
    (
        "defaultPreferredDirectionTraceCost",
        &["default_preferred_direction_trace_cost"],
    ),
    (
        "defaultUndesiredDirectionTraceCost",
        &["default_undesired_direction_trace_cost"],
    ),
    ("viaCosts", &["via_costs", "viaCosts"]),
    ("planeViaCosts", &["plane_via_costs"]),
    ("startRipupCosts", &["start_ripup_costs", "startRipupCosts"]),
    ("defaultBendCost", &["default_bend_cost"]),
    ("unroutedNetPenalty", &["unrouted_net_penalty"]),
    ("clearanceViolationPenalty", &["clearance_violation_penalty"]),
    ("bendPenalty", &["bend_penalty"]),
];

fn set_scoring(s: &mut RoutingCostSettings, path: &[&str], v: &str) -> R {
    let f = find_field(SCORING_FIELDS, path[0])?;
    leaf(path, f)?;
    match f {
        "preferredDirectionTraceCost" => set_doubles(&mut s.preferred_direction_trace_cost, f, v),
        "undesiredDirectionTraceCost" => set_doubles(&mut s.undesired_direction_trace_cost, f, v),
        "defaultPreferredDirectionTraceCost" => {
            set_f64(&mut s.default_preferred_direction_trace_cost, f, v)
        }
        "defaultUndesiredDirectionTraceCost" => {
            set_f64(&mut s.default_undesired_direction_trace_cost, f, v)
        }
        "viaCosts" => set_i32(&mut s.via_costs, f, v),
        "planeViaCosts" => set_i32(&mut s.plane_via_costs, f, v),
        "startRipupCosts" => set_i32(&mut s.start_ripup_costs, f, v),
        "defaultBendCost" => set_f64(&mut s.default_bend_cost, f, v),
        "unroutedNetPenalty" => set_f32(&mut s.unrouted_net_penalty, f, v),
        "clearanceViolationPenalty" => set_f32(&mut s.clearance_violation_penalty, f, v),
        _ => set_f32(&mut s.bend_penalty, f, v),
    }
}

const ROUTER_SCORING_FIELDS: &[FieldDesc] = &[
    ("version", &["version"]),
    ("unroutedConnectionWeight", &["unrouted_connection_weight"]),
    ("unroutedFreeFraction", &["unrouted_free_fraction"]),
    ("unroutedFirstHalfWeight", &["unrouted_first_half_weight"]),
    ("unroutedSecondHalfWeight", &["unrouted_second_half_weight"]),
    ("clearanceViolationCountWeight", &["clearance_violation_count_weight"]),
    ("clearanceViolationDepthWeight", &["clearance_violation_depth_weight"]),
    ("clearanceViolationDepthScale", &["clearance_violation_depth_scale"]),
];

fn set_router_scoring(s: &mut RouterScoreSettings, path: &[&str], v: &str) -> R {
    let f = find_field(ROUTER_SCORING_FIELDS, path[0])?;
    leaf(path, f)?;
    match f {
        "version" => set_enum(&mut s.version, f, v),
        "unroutedConnectionWeight" => set_f32(&mut s.unrouted_connection_weight, f, v),
        "unroutedFreeFraction" => set_f32(&mut s.unrouted_free_fraction, f, v),
        "unroutedFirstHalfWeight" => set_f32(&mut s.unrouted_first_half_weight, f, v),
        "unroutedSecondHalfWeight" => set_f32(&mut s.unrouted_second_half_weight, f, v),
        "clearanceViolationCountWeight" => set_f32(&mut s.clearance_violation_count_weight, f, v),
        "clearanceViolationDepthWeight" => set_f32(&mut s.clearance_violation_depth_weight, f, v),
        _ => set_f32(&mut s.clearance_violation_depth_scale, f, v),
    }
}

const OPTIMIZER_SCORING_FIELDS: &[FieldDesc] = &[
    ("version", &["version"]),
    ("excessWireLengthWeight", &["excess_wire_length_weight"]),
    ("excessViaWeight", &["excess_via_weight"]),
    ("excessBendWeight", &["excess_bend_weight"]),
    ("lengthFloor", &["length_floor"]),
    ("difficultyScaleFloor", &["difficulty_scale_floor"]),
];

fn set_optimizer_scoring(s: &mut OptimizerScoreSettings, path: &[&str], v: &str) -> R {
    let f = find_field(OPTIMIZER_SCORING_FIELDS, path[0])?;
    leaf(path, f)?;
    match f {
        "version" => set_enum(&mut s.version, f, v),
        "excessWireLengthWeight" => set_f32(&mut s.excess_wire_length_weight, f, v),
        "excessViaWeight" => set_f32(&mut s.excess_via_weight, f, v),
        "excessBendWeight" => set_f32(&mut s.excess_bend_weight, f, v),
        "lengthFloor" => set_f32(&mut s.length_floor, f, v),
        _ => set_f32(&mut s.difficulty_scale_floor, f, v),
    }
}

const LAYER_FIELDS: &[FieldDesc] = &[
    ("routable", &["routable"]),
    ("preferredDirectionHorizontal", &["preferred_direction_horizontal"]),
    ("bendCost", &["bend_cost"]),
    ("preferredDirectionTraceCost", &["preferred_direction_trace_cost"]),
    ("undesiredDirectionTraceCost", &["undesired_direction_trace_cost"]),
];

fn set_layer(s: &mut LayerSettings, path: &[&str], v: &str) -> R {
    let f = find_field(LAYER_FIELDS, path[0])?;
    leaf(path, f)?;
    match f {
        "routable" => set_bool(&mut s.routable, v),
        "preferredDirectionHorizontal" => set_bool(&mut s.preferred_direction_horizontal, v),
        "bendCost" => set_f64(&mut s.bend_cost, f, v),
        "preferredDirectionTraceCost" => set_f64(&mut s.preferred_direction_trace_cost, f, v),
        _ => set_f64(&mut s.undesired_direction_trace_cost, f, v),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{ItemSelectionStrategy, RouterScoringVersion};

    #[test]
    fn paths() {
        let mut s = RouterSettings::new();
        set_field_value(&mut s, "autorouter.max_passes", "7").unwrap();
        set_field_value(&mut s, "optimizer.improvement_threshold", "3.5").unwrap();
        set_field_value(&mut s, "optimizer.itemSelectionStrategy", " prioritized").unwrap();
        set_field_value(&mut s, "routerScoring.version", "V1_LEGACY").unwrap();
        set_field_value(&mut s, "scoring.viaCosts", "80").unwrap();
        set_field_value(&mut s, "plane_nets", " GND , VCC ").unwrap();
        set_field_value(&mut s, "conduction-is-obstacle", "1").unwrap_err();
        set_field_value(&mut s, "conduction_is_obstacle", "1").unwrap();
        set_field_value(&mut s, "layers.routable", "false,true").unwrap();
        set_field_value(&mut s, "layers.bend_cost", "1,2,3").unwrap();
        assert_eq!(s.autorouter.max_passes, Some(7));
        assert_eq!(s.optimizer.optimization_improvement_threshold, Some(3.5));
        assert_eq!(
            s.optimizer.item_selection_strategy,
            Some(ItemSelectionStrategy::Prioritized)
        );
        assert_eq!(s.router_scoring.version, Some(RouterScoringVersion::V1Legacy));
        assert_eq!(s.scoring.via_costs, Some(80));
        assert_eq!(s.plane_nets, Some(vec!["GND".into(), "VCC".into()]));
        assert_eq!(s.plane_as_obstacle, Some(true));
        let layers = s.layers.unwrap();
        assert_eq!(layers.len(), 2);
        assert_eq!(layers[0].routable, Some(false));
        assert_eq!(layers[1].bend_cost, Some(2.0));
        let mut s = RouterSettings::new();
        assert!(matches!(
            set_field_value(&mut s, "via_costs", "150"),
            Err(SetFieldError::NoSuchField(_))
        ));
        assert!(set_field_value(&mut s, "autorouter.max_passes", "x").is_err());
    }
}
