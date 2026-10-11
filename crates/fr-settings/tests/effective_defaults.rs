use fr_settings::*;

/// The complete effective default `RouterSettings` after `SettingsMerger.merge()`
/// with only `DefaultSettings` (8 available processors).
fn expected_defaults() -> RouterSettings {
    RouterSettings {
        autorouter: AutorouterSettings {
            enabled: Some(true),
            algorithm: Some("freerouting-router".into()),
            max_passes: Some(0),
            min_passes: None,
            max_items: Some(i32::MAX),
            max_threads: Some(7),
            save_intermediate_stages: Some(false),
            ignore_net_classes: Some(vec![]),
        },
        fanout: FanoutSettings {
            enabled: Some(true),
            max_passes: Some(20),
            max_items: Some(i32::MAX),
            max_milliseconds_per_pin: Some(10000),
            ripup_allowed: Some(true),
            min_escape_length_mm: Some(2.5),
            max_escape_length_mm: Some(4.5),
            start_via_diameter_mm: Some(0.25),
            end_via_diameter_mm: Some(0.25),
            pin_sorting_order: Some("outer_first".into()),
            fallback_to_board_vias: Some(true),
            timeout_string: None,
        },
        copper_to_edge_clearance_um: Some(250.0),
        hole_clearance_um: Some(0.0),
        same_net_via_hole_to_hole_um: None,
        clearance_exempt_areas: None,
        clearance_tolerance_um: Some(1.0),
        plane_nets: Some(vec![]),
        plane_as_obstacle: Some(false),
        neck_width_um: Some(0.0),
        min_trace_width_um: None,
        strict_drc: Some(false),
        job_timeout_string: Some("12:00:00".into()),
        layers: None,
        validation_warnings: Some(vec![]),
        trace_pull_tight_accuracy: Some(500),
        vias_allowed: Some(true),
        automatic_neckdown: Some(true),
        optimizer: OptimizerSettings {
            enabled: Some(true),
            algorithm: Some("freerouting-optimizer".into()),
            max_passes: Some(100),
            max_items: Some(i32::MAX),
            max_threads: Some(7),
            optimization_improvement_threshold: Some(2.5),
            enable_preflight_guards: Some(true),
            max_consecutive_failures: Some(50),
            max_consecutive_failures_pass1: Some(12),
            additional_ripup_cost_factor_at_start: Some(10),
            trace_ripup_cost_factor: Some(0.6),
            max_autoroute_passes: Some(6),
            board_update_strategy: Some(BoardUpdateStrategy::GlobalOptimal),
            item_selection_strategy: Some(ItemSelectionStrategy::Sequential),
            timeout_string: None,
        },
        scoring: RoutingCostSettings {
            preferred_direction_trace_cost: None,
            undesired_direction_trace_cost: None,
            default_preferred_direction_trace_cost: Some(1.0),
            default_undesired_direction_trace_cost: Some(1.0),
            via_costs: Some(50),
            plane_via_costs: Some(5),
            start_ripup_costs: Some(100),
            default_bend_cost: Some(0.0),
            unrouted_net_penalty: Some(5_000_000.0),
            clearance_violation_penalty: Some(1_000_000.0),
            bend_penalty: Some(10.0),
        },
        router_scoring: RouterScoreSettings {
            version: Some(RouterScoringVersion::V2Continuous),
            unrouted_connection_weight: Some(1000.0),
            unrouted_free_fraction: Some(0.5),
            unrouted_first_half_weight: Some(1000.0f32 / 3.0f32),
            unrouted_second_half_weight: Some(2000.0f32 / 3.0f32),
            clearance_violation_count_weight: Some(25.0),
            clearance_violation_depth_weight: Some(300.0),
            clearance_violation_depth_scale: Some(1000.0),
        },
        optimizer_scoring: OptimizerScoreSettings {
            version: Some(OptimizerScoringVersion::V2LowerBound),
            excess_wire_length_weight: Some(1000.0),
            excess_via_weight: Some(2000.0),
            excess_bend_weight: Some(500.0),
            length_floor: Some(1.0),
            difficulty_scale_floor: Some(1.0),
        },
        max_threads: Some(7),
        result_json_path: None,
        board_specific_trace_costs_applied: None,
    }
}

#[test]
fn effective_default_router_settings() {
    let merged = SettingsMerger::new([SettingsSource::new(
        SourceKind::Default,
        "Default Settings",
        default_settings(8),
    )])
    .merge(8);
    assert_eq!(merged, expected_defaults());
    assert_eq!(parse_timespan_string(merged.job_timeout_string.as_deref().unwrap()), Some(43200));
    assert_eq!(merged.get_start_ripup_costs(), 100);
    assert_eq!(merged.get_via_costs(), 50);
    assert_eq!(merged.get_plane_via_costs(), 5);
    assert!(merged.get_run_router() && merged.get_run_optimizer() && merged.get_run_fanout());
    // f32 bit patterns of the thirds.
    assert_eq!(merged.router_scoring.unrouted_first_half_weight.unwrap().to_bits(), 0x43a6_aaab);
    assert_eq!(merged.router_scoring.unrouted_second_half_weight.unwrap().to_bits(), 0x4426_aaab);
}

#[test]
fn single_processor_threads() {
    let merged = SettingsMerger::new([SettingsSource::new(
        SourceKind::Default,
        "d",
        default_settings(1),
    )])
    .merge(1);
    assert_eq!(merged.max_threads, Some(1));
    assert_eq!(merged.optimizer.max_threads, Some(1));
    assert_eq!(merged.autorouter.max_threads, Some(1));
}

#[test]
fn headless_merge_with_dsn_and_board() {
    let dsn = fr_dsn::Dsn::parse(
        b"(pcb b (structure (layer F.Cu (type signal)) (layer In1.Cu (type signal)) \
          (layer In2.Cu (type signal)) (layer B.Cu (type signal)) \
          (boundary (rect pcb 0 0 2000 1000))))",
    )
    .unwrap();
    let dsn_settings = DsnFileSettings::from_dsn(&dsn);
    assert!(dsn_settings.autoroute_settings.is_none());
    let cli = CliSettings::parse(&["-de", "b.dsn", "-do", "b.ses", "--router.optimizer.enabled=false"]);
    let env = EnvironmentSettings::parse(Vec::<(String, String)>::new());
    let mut s = headless_merger(&cli, &env, Some(&dsn_settings), None, 8).merge(8);
    assert_eq!(s.get_layer_count(), 4);
    assert_eq!(s.board_specific_trace_costs_applied, Some(false));
    assert!(!s.get_run_optimizer());

    s.apply_board_specific_optimizations(&BoardLayerInfo {
        bounding_box_width: 2000,
        bounding_box_height: 1000,
        layer_is_signal: &[true, true, true, true],
    });
    let h_add = 0.1 * 20.0;
    let v_add = 0.1 * 5.0;
    let outer = 0.2 * 4.0;
    let costs = s.get_trace_costs();
    // Wider than high: layer 0 horizontal, then alternating.
    assert!(s.get_preferred_direction_is_horizontal(0));
    assert!(!s.get_preferred_direction_is_horizontal(1));
    assert_eq!(costs[0].horizontal, 1.0 + outer);
    assert_eq!(costs[0].vertical, 1.0 + h_add + outer);
    assert_eq!(costs[1].vertical, 1.0);
    assert_eq!(costs[1].horizontal, 1.0 + v_add);
    assert_eq!(costs[3].vertical, 1.0 + outer);
    assert_eq!(costs[3].horizontal, 1.0 + v_add + outer);
    assert_eq!(s.board_specific_trace_costs_applied, Some(true));
    // Second application keeps the costs.
    let before = s.clone();
    s.apply_board_specific_optimizations_if_needed(&BoardLayerInfo {
        bounding_box_width: 2000,
        bounding_box_height: 1000,
        layer_is_signal: &[true, true, true, true],
    });
    assert_eq!(s, before);
}

#[test]
fn power_layer_not_routable_and_exclusions() {
    let mut s = default_settings(4);
    s.set_layer_count(3);
    s.autorouter.ignore_net_classes = Some(vec!["gnd".into(), " ".into()]);
    s.apply_board_specific_optimizations(&BoardLayerInfo {
        bounding_box_width: 100,
        bounding_box_height: 300,
        layer_is_signal: &[true, false, true],
    });
    assert!(!s.get_layer_active(1));
    assert!(s.get_layer_active(0));
    // Higher than wide: first signal layer vertical.
    assert!(!s.get_preferred_direction_is_horizontal(0));
    assert!(s.get_preferred_direction_is_horizontal(2));
    assert_eq!(s.net_class_exclusions(&["default", "GND", "Gnd"]), vec![1, 2]);
    s.validate_against_board(Some(&["default"]), 4);
    assert_eq!(s.validation_warnings.as_ref().unwrap().len(), 1);
}

#[test]
fn dsn_scope_overrides_and_priorities() {
    let dsn = fr_dsn::Dsn::parse(
        b"(pcb b (structure (layer F.Cu (type signal)) (layer B.Cu (type signal)) \
          (autoroute_settings (postroute on) (via_costs 70) (layer_rule B.Cu (active off)))))",
    )
    .unwrap();
    let d = DsnFileSettings::from_dsn(&dsn);
    // CLI (60) beats DSN (20) in the merge ...
    let cli = CliSettings::parse(&["--router.scoring.via_costs=20"]);
    let env = EnvironmentSettings::default();
    let mut s = headless_merger(&cli, &env, Some(&d), None, 8).merge(8);
    assert_eq!(s.get_via_costs(), 20);
    assert!(!s.get_layer_active(1));
    // ... but the board load re-applies the DSN scope on top.
    s.apply_new_values_from(d.autoroute_settings.as_ref().unwrap());
    assert_eq!(s.get_via_costs(), 70);
}
