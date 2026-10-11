//! Headless post-load processing (Java `management.HeadlessBoardManager`
//! `applyParsedBoardResult` -> `applyRouterSettingsForLoadedBoard` +
//! `applyImmediatePostLoadProcessing`, and `RoutingBoard.reduceNetsOfRouteItems` /
//! `changePlaneAsObstacle`) and the board preparation of `RoutingJobScheduler`.
//!
//! Java sequence for a headless DSN job (`RoutingJobScheduler`):
//!
//! 1. `HeadlessBoardManager.loadFromSpecctraDsn`: `DsnReader.readBoard` ([`crate::load`] +
//!    [`crate::build_board`]), then with the job's settings as created by `Freerouting`
//!    ([`load_from_specctra_dsn`]):
//!    `applyRouterSettingsForLoadedBoard` ([`apply_router_settings_for_loaded_board`]) and
//!    `applyImmediatePostLoadProcessing` ([`apply_immediate_post_load_processing`]).
//!    The deferred part (`scheduleDeferredPostLoadProcessing`) runs on a virtual thread; its
//!    only board effect is `preExistingClearanceViolationsCount` /
//!    `unfixableClearanceViolationsCount` from a full DRC (U6), racing with the router start.
//! 2. The scheduler re-merges the settings (Dsn, Rules, then the job settings as Api source)
//!    — fr-settings' `SettingsMerger` — and applies a RULES file (`RulesReader`, not ported).
//! 3. [`prepare_for_routing`]: `applyBoardSpecificOptimizations` and `applyNetClassExclusions`
//!    on the merged settings, then an initial SES session if given ([`crate::ses_reader`]).
//!
//! The DSN `(autoroute_settings ...)` scope is **not** applied onto the job settings during the
//! load: `Structure.readScope` only does that when the board manager has a current routing
//! job, and `DsnReader.readBoard` uses `ReadScopeParameter.MinimalBoardManager`, whose
//! `getCurrentRoutingJob()` returns `null`. The scope only reaches the job through
//! `DsnFileSettings` in the settings merger.
//!
//! The validations of `applyImmediatePostLoadProcessing` (`validatePowerPlanes`,
//! `validateBoardDesignErrors`) only log and are not ported.

use fr_engine::board::{BasicBoard, ItemKey, ItemKind, ObstacleKind, RoutingBoard};
use fr_engine::ids::NetNo;
use fr_engine::rules::ItemClass;
use fr_engine::structure::Unit;
use fr_geom::{Area, Shape};
use fr_settings::defaults::DEFAULT_COPPER_TO_EDGE_CLEARANCE_UM;
use fr_settings::settings::{BoardLayerInfo, RouterSettings};

use crate::error::LoadError;

const BOARD_EDGE_CLEARANCE_CLASS_NAME: &str = "board_edge";
const HOLE_EDGE_CLEARANCE_CLASS_NAME: &str = "hole_edge";

fn round_i32(x: f64) -> i32 {
    fr_jcompat::math_round_i32(x)
}

/// `(int) Math.round(Unit.scale(um * max(1, resolution), UM, board unit))`.
fn um_to_board_units(board: &BasicBoard, um: f64) -> i32 {
    let resolution = board.communication.resolution.max(1);
    round_i32(Unit::scale(
        um * resolution as f64,
        Unit::Um,
        board.communication.unit,
    ))
}

fn default_area_class(board: &BasicBoard) -> i32 {
    let d = board.rules.default_net_class();
    board.rules.net_classes[d]
        .default_item_clearance_classes
        .get(ItemClass::Area)
}

/// `RouterSettings.applyBoardSpecificOptimizations(board)`.
pub fn apply_board_specific_optimizations(board: &BasicBoard, settings: &mut RouterSettings) {
    let signal: Vec<bool> = board
        .layer_structure
        .layers
        .iter()
        .map(|l| l.is_signal)
        .collect();
    let info = BoardLayerInfo {
        bounding_box_width: board.bounding_box.width(),
        bounding_box_height: board.bounding_box.height(),
        layer_is_signal: &signal,
    };
    settings.apply_board_specific_optimizations(&info);
}

/// `RouterSettings.applyNetClassExclusions(board)`.
pub fn apply_net_class_exclusions(board: &mut BasicBoard, settings: &RouterSettings) {
    let rules = &board.rules;
    let names: Vec<String> = rules
        .net_classes
        .iter()
        .map(|c| rules.net_classes[c].get_name().to_string())
        .collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let marked = settings.net_class_exclusions(&refs);
    if marked.is_empty() {
        return;
    }
    let rules = board.rules_mut();
    for i in marked {
        let c = rules.net_classes.get(i as i32);
        rules.net_classes[c].is_ignored_by_autorouter = true;
    }
}

/// `HeadlessBoardManager.applyCopperToEdgeClearanceOverride`.
pub fn apply_copper_to_edge_clearance_override(board: &mut BasicBoard, settings: &RouterSettings) {
    let Some(um) = settings.copper_to_edge_clearance_um else {
        return;
    };
    if um < 0.0 {
        log::warn!("Ignoring router.copper_to_edge_clearance_um because it is negative: {um}");
        return;
    }
    let Some(outline) = board.get_outline() else {
        log::warn!("Ignoring router.copper_to_edge_clearance_um: no board outline");
        return;
    };
    let uses_fallback_class = board.item(outline).clearance_class() == default_area_class(board);
    let uses_default_value = (um - DEFAULT_COPPER_TO_EDGE_CLEARANCE_UM).abs() < 1e-9;
    if uses_default_value && !uses_fallback_class {
        return;
    }
    let value = um_to_board_units(board, um);
    let matrix = &mut board.rules_mut().clearance_matrix;
    let mut class_no = matrix.get_no(BOARD_EDGE_CLEARANCE_CLASS_NAME);
    if class_no < 0 {
        matrix.append_class(BOARD_EDGE_CLEARANCE_CLASS_NAME);
        class_no = matrix.get_no(BOARD_EDGE_CLEARANCE_CLASS_NAME);
    }
    if class_no < 0 {
        return;
    }
    for layer in 0..matrix.get_layer_count() {
        for c in 1..matrix.get_class_count() {
            matrix.set_value(class_no, c, layer, value);
            matrix.set_value(c, class_no, layer, value);
        }
    }
    board.tree_remove(outline);
    board.set_clearance_class_index(outline, class_no);
    board.clear_derived_data(outline);
    board.tree_insert(outline);
}

/// `HeadlessBoardManager.assignHoleKeepoutClearanceClass`: returns the number of reclassified
/// keepouts.
fn assign_hole_keepout_clearance_class(board: &mut BasicBoard, hole_clearance: i32) -> usize {
    let hole_keepouts: Vec<ItemKey> = board
        .get_items()
        .into_iter()
        .filter(|&k| {
            let it = board.item(k);
            match &it.kind {
                ItemKind::ObstacleArea(a) if a.kind == ObstacleKind::Keepout => {
                    it.component_no() > 0
                        && matches!(&*a.get_area(board), Area::Shape(Shape::Circle(_)))
                }
                _ => false,
            }
        })
        .collect();
    if hole_keepouts.is_empty() {
        return 0;
    }
    let default_area = default_area_class(board);
    let matrix = &mut board.rules_mut().clearance_matrix;
    let mut class_no = matrix.get_no(HOLE_EDGE_CLEARANCE_CLASS_NAME);
    if class_no < 0 {
        matrix.append_class(HOLE_EDGE_CLEARANCE_CLASS_NAME);
        class_no = matrix.get_no(HOLE_EDGE_CLEARANCE_CLASS_NAME);
    }
    if class_no < 0 {
        return 0;
    }
    for layer in 0..matrix.get_layer_count() {
        for c in 1..matrix.get_class_count() {
            let v = hole_clearance.max(matrix.get_value(default_area, c, layer, false));
            matrix.set_value(class_no, c, layer, v);
            matrix.set_value(c, class_no, layer, v);
        }
    }
    let mut reclassified = 0;
    for k in hole_keepouts {
        if board.item(k).clearance_class() != class_no {
            board.set_clearance_class_index(k, class_no);
            board.clear_derived_data(k);
            reclassified += 1;
        }
    }
    reclassified
}

/// fastroute extension: `router.same_net_via_hole_to_hole_um` into the board rules (clearance check only).
pub fn apply_same_net_via_hole_to_hole(board: &mut BasicBoard, settings: &RouterSettings) {
    let Some(um) = settings.same_net_via_hole_to_hole_um else {
        return;
    };
    if um < 0.0 {
        log::warn!(target: "fastroute", "Ignoring router.same_net_via_hole_to_hole_um because it is negative: {um}");
        return;
    }
    let value = um_to_board_units(board, um) as f64;
    board.rules_mut().set_same_net_via_hole_to_hole(Some(value));
    log::info!(target: "fastroute", "same-net via pairs judged by hole-to-hole {um} um (clearance check)");
}

/// fastroute extension: `router.clearance_exempt_areas` ("CLASS@x0:y0:x1:y1@base_um", DSN units).
pub fn apply_clearance_exempt_areas(board: &mut BasicBoard, settings: &RouterSettings) {
    let Some(entries) = &settings.clearance_exempt_areas else {
        return;
    };
    let mut out = Vec::new();
    for e in entries {
        let parts: Vec<&str> = e.split('@').collect();
        let nums: Vec<f64> = parts.get(1).map(|b| b.split(':').filter_map(|x| x.parse().ok()).collect()).unwrap_or_default();
        let base: Option<f64> = parts.get(2).and_then(|b| b.parse().ok());
        if parts.len() != 3 || nums.len() != 4 || base.is_none() {
            log::warn!(target: "fastroute", "Ignoring router.clearance_exempt_areas entry '{e}' (want CLASS@x0:y0:x1:y1@base_um)");
            continue;
        }
        let ct = &board.communication.coordinate_transform;
        let a = ct.dsn_to_board_point(&[nums[0], nums[1]]);
        let b = ct.dsn_to_board_point(&[nums[2], nums[3]]);
        let bx = fr_geom::IntBox::new(a.x.min(b.x).floor() as i32, a.y.min(b.y).floor() as i32,
                                      a.x.max(b.x).ceil() as i32, a.y.max(b.y).ceil() as i32);
        out.push((parts[0].to_string(), bx, um_to_board_units(board, base.unwrap()) as f64));
    }
    // target "fastroute": the default logger shows only its own targets, and a dropped entry must be seen
    log::info!(target: "fastroute", "{} clearance exempt area(s) (clearance check)", out.len());
    board.rules_mut().clearance_exempt_areas = out;
}

/// `HeadlessBoardManager.applyHoleClearanceOverride`.
pub fn apply_hole_clearance_override(board: &mut BasicBoard, settings: &RouterSettings) {
    let Some(um) = settings.hole_clearance_um else {
        return;
    };
    if um < 0.0 {
        log::warn!("Ignoring router.hole_clearance_um because it is negative: {um}");
        return;
    }
    let value = um_to_board_units(board, um);
    let changed = value != board.rules.get_hole_clearance();
    board.rules_mut().set_hole_clearance(value);
    let mut hole_keepouts = 0;
    if value > 0 {
        hole_keepouts = assign_hole_keepout_clearance_class(board, value);
    }
    if changed || hole_keepouts > 0 {
        board.reinsert_tree_items();
    }
}

/// `HeadlessBoardManager.applyPlaneNetsOverride`.
pub fn apply_plane_nets_override(board: &mut BasicBoard, settings: &RouterSettings) {
    let Some(names) = &settings.plane_nets else {
        return;
    };
    for name in names {
        // Java String.isBlank / trim (both on chars <= ' ' / whitespace; ASCII suffices here)
        let trimmed = name.trim_matches(|c: char| c <= ' ');
        if trimmed.is_empty() {
            continue;
        }
        let nets: Vec<NetNo> = board
            .rules
            .nets
            .get_all_by_name(trimmed)
            .iter()
            .map(|n| n.net_number)
            .collect();
        for n in nets {
            let net = board.rules_mut().nets.get_mut(n).expect("net");
            if !net.contains_plane() {
                net.set_contains_plane(true);
            }
        }
    }
}

/// `HeadlessBoardManager.applyPlaneAsObstacleOverride`.
pub fn apply_plane_as_obstacle_override(board: &mut RoutingBoard, settings: &RouterSettings) {
    if let Some(v) = settings.plane_as_obstacle {
        board.change_plane_as_obstacle(v);
    }
}

/// `HeadlessBoardManager.applyClearanceToleranceOverride` (the DRC settings copy is the
/// caller's).
pub fn apply_clearance_tolerance_override(board: &mut BasicBoard, settings: &RouterSettings) {
    let Some(t) = settings.clearance_tolerance_um else {
        return;
    };
    if !t.is_finite() || t < 0.0 {
        log::warn!("Ignoring router.clearance_tolerance_um because it is invalid: {t}");
        return;
    }
    board.rules_mut().clearance_tolerance_um = t;
}

/// `HeadlessBoardManager.applyRouterSettingsForLoadedBoard`.
pub fn apply_router_settings_for_loaded_board(
    board: &mut RoutingBoard,
    settings: &mut RouterSettings,
) {
    let n = board.layer_structure.layer_count() as usize;
    if settings.get_layer_count() != n {
        settings.set_layer_count(n);
    }
    apply_board_specific_optimizations(board, settings);
    apply_net_class_exclusions(board, settings);
    apply_copper_to_edge_clearance_override(board, settings);
    apply_hole_clearance_override(board, settings);
    apply_same_net_via_hole_to_hole(board, settings);
    apply_clearance_exempt_areas(board, settings);
    apply_plane_nets_override(board, settings);
    apply_plane_as_obstacle_override(board, settings);
    apply_clearance_tolerance_override(board, settings);
}

/// `HeadlessBoardManager.applyImmediatePostLoadProcessing` (without the logging-only
/// validations).
pub fn apply_immediate_post_load_processing(board: &mut RoutingBoard) {
    board.expand_bounding_box_to_include_all_items();
    board.reduce_nets_of_route_items();
}

/// `HeadlessBoardManager.loadFromSpecctraDsn`: `DsnReader.readBoard` plus the immediate
/// post-load processing with the job's settings as `Freerouting` created them.
pub fn load_from_specctra_dsn(
    src: &[u8],
    settings: &mut RouterSettings,
) -> Result<RoutingBoard, LoadError> {
    let design = crate::load_bytes(src)?;
    let mut board = RoutingBoard::from_basic(crate::build_board(design));
    apply_router_settings_for_loaded_board(&mut board, settings);
    apply_immediate_post_load_processing(&mut board);
    Ok(board)
}

/// The board part of `RoutingJobScheduler` after the settings re-merge (RULES files are not
/// ported): `applyBoardSpecificOptimizations`, `applyNetClassExclusions` on the merged
/// settings, then the initial session, if any.
pub fn prepare_for_routing(
    board: &mut RoutingBoard,
    merged: &mut RouterSettings,
    initial_session: Option<&[u8]>,
) -> Option<crate::ses_reader::SesImportSummary> {
    apply_board_specific_optimizations(board, merged);
    apply_net_class_exclusions(board, merged);
    initial_session.map(|s| crate::ses_reader::read_ses(s, board))
}
