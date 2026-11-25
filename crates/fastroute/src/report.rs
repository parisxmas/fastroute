//! `--report=FILE`: a JSON summary of a run (result statistics, timings, the unrouted
//! connections and the clearance violations), for benchmarks and for finding out why
//! connections stay unrouted.
//!
//! With `--diagnose`, every unrouted connection is routed once more on its own, on the board
//! as it was loaded (pins, keepouts and fixed wiring, no routed traces). If one of its ends
//! routes there, the connection failed because of the other traces (`congestion`); if neither
//! end routes, the loaded geometry or the rules block it (`blocked`). The attempt connects the
//! end to the nearest part of its net, which for nets with more than two pins need not be the
//! other end of the airline. Each connection is also routed on the final board as autorouter
//! passes 1 and 10 would (rip-up allowed): `in_context` gives the attempt state, its message and
//! how many items it ripped, and the unrouted count before, after it and after the tail removal
//! that ends a pass.

use std::fmt::Write as _;

use fr_engine::board::{AutorouteAttemptState, BasicBoard, ItemKey, ItemKind, RoutingBoard};
use fr_engine::drc::{all_clearance_violations, Category, DesignRulesChecker};
use fr_engine::scoring::statistics::BoardStatistics;
use fr_settings::RouterSettings;

pub struct Timings {
    pub load_s: f64,
    pub route_s: f64,
    pub total_s: f64,
}

/// The result of routing one end of an unrouted connection on the loaded board.
struct Attempt {
    state: AutorouteAttemptState,
    details: String,
}

fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => write!(out, "\\u{:04x}", c as u32).unwrap(),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn num(v: f64) -> String {
    if v.is_finite() {
        format!("{v:.4}").trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        "null".into()
    }
}

fn opt_i(v: Option<i32>) -> String {
    v.map(|x| x.to_string()).unwrap_or_else(|| "null".into())
}

struct Units<'a> {
    board: &'a BasicBoard,
    per_mm: f64,
}

impl Units<'_> {
    fn x(&self, x: f64) -> String {
        num(x / self.per_mm)
    }
    /// Specctra y points up; KiCad's down.
    fn y(&self, y: f64) -> String {
        num(-y / self.per_mm)
    }
    fn layer(&self, l: i32) -> String {
        json_str(&self.board.layer_structure.layers.get(l as usize).map(|x| x.name.clone()).unwrap_or_else(|| format!("L{l}")))
    }
}

/// The cause of a clearance violation (`ClearanceViolation.category`). `pin_to_pin` and
/// `pin_to_outline_or_keepout` come from the footprints themselves: no routing change removes
/// them. `fixed_route` sits on an item that was already placed when the run started.
pub(crate) fn category_name(category: Category) -> &'static str {
    match category {
        Category::PinToPin => "pin_to_pin",
        Category::PinToOutlineOrKeepout => "pin_to_outline_or_keepout",
        Category::FixedRoute => "fixed_route",
        Category::OtherUnfixable => "other_unfixable",
        Category::PotentiallyFixable => "potentially_fixable",
    }
}

/// `{"kind": .., "component": .., "pin": .., "layers": [..]}` of an item.
fn item_json(u: &Units, key: ItemKey) -> String {
    let board = u.board;
    let item = board.item(key);
    let kind = match &item.kind {
        ItemKind::Pin(_) => "pin",
        ItemKind::Via(_) => "via",
        ItemKind::Trace(_) => "trace",
        ItemKind::ObstacleArea(_) => "keepout",
        ItemKind::ConductionArea(_) => "plane",
        ItemKind::ComponentOutline(_) => "outline",
        ItemKind::BoardOutline(_) => "board_outline",
    };
    let mut s = format!("{{\"kind\": \"{kind}\"");
    if item.component_no() > 0 {
        let comp = board.components.get(item.component_no());
        write!(s, ", \"component\": {}", json_str(&comp.name)).unwrap();
        if let ItemKind::Pin(p) = &item.kind {
            let pkg = board.library.packages.get(comp.get_package());
            if let Some(pin) = pkg.get_pin(p.pin_index) {
                write!(s, ", \"pin\": {}", json_str(&pin.name)).unwrap();
            }
        }
    }
    write!(s, ", \"layers\": [{}, {}]}}", u.layer(item.first_layer(board)), u.layer(item.last_layer(board))).unwrap();
    s
}

fn state_name(s: AutorouteAttemptState) -> String {
    format!("{s:?}")
}

/// Routes `item` (by id) alone on a copy of `loaded`.
fn attempt(loaded: &RoutingBoard, board: &BasicBoard, key: ItemKey, settings: &RouterSettings) -> Option<Attempt> {
    let id = board.item(key).id();
    let mut b = loaded.clone();
    let k = b.get_item(id)?;
    let r = b.autoroute(k, settings, settings.get_via_costs(), None, None);
    Some(Attempt { state: r.state, details: r.details })
}

fn attempt_json(a: &Option<Attempt>) -> String {
    match a {
        None => "null".into(),
        Some(a) => format!("{{\"state\": {}, \"details\": {}}}", json_str(&state_name(a.state)), json_str(&a.details)),
    }
}

/// Writes the report. `loaded` is the board before routing (only needed for `--diagnose`).
pub fn write_report(
    path: &str,
    input: &str,
    board: &RoutingBoard,
    loaded: Option<&RoutingBoard>,
    settings: &RouterSettings,
    unclamped_optimizer_score: bool,
    enhancements: bool,
    timings: &Timings,
) -> std::io::Result<()> {
    let t = std::time::Instant::now();
    let basic: &BasicBoard = board;
    let res = board.communication.resolution.max(1) as f64;
    let u = Units { board: basic, per_mm: res * 1000.0 };
    let mut stats = BoardStatistics::from_board(basic);
    let router_score = stats.get_router_score(Some(settings));
    let optimizer_score = if unclamped_optimizer_score {
        stats.get_optimizer_score_unclamped(Some(settings))
    } else {
        stats.get_optimizer_score(Some(settings))
    };

    let mut checker = DesignRulesChecker::new();
    let airlines = checker.get_all_airlines(basic);
    let violations = all_clearance_violations(basic);

    let mut unrouted = Vec::new();
    let (mut congestion, mut blocked, mut ignored) = (0, 0, 0);
    for a in &airlines {
        let net = basic.rules.nets.get(a.net_number).map(|n| n.name.clone()).unwrap_or_default();
        let is_ignored =
            basic.rules.nets.get(a.net_number).is_some_and(|n| basic.rules.net_classes[n.get_net_class()].is_ignored_by_autorouter);
        let dx = a.to_corner.x - a.from_corner.x;
        let dy = a.to_corner.y - a.from_corner.y;
        let mut s = format!(
            "    {{\"net\": {}, \"length_mm\": {}, \"from\": {}, \"from_xy\": [{}, {}], \"to\": {}, \"to_xy\": [{}, {}]",
            json_str(&net),
            num((dx * dx + dy * dy).sqrt() / u.per_mm),
            item_json(&u, a.from_item),
            u.x(a.from_corner.x),
            u.y(a.from_corner.y),
            item_json(&u, a.to_item),
            u.x(a.to_corner.x),
            u.y(a.to_corner.y),
        );
        if is_ignored {
            // net class ignored by the autorouter: never routed, nothing to diagnose
            ignored += 1;
            if loaded.is_some() {
                s.push_str(", \"diagnosis\": {\"class\": \"ignored\"}");
            }
        } else if let Some(loaded) = loaded {
            let from = attempt(loaded, basic, a.from_item, settings);
            let routed = |x: &Option<Attempt>| x.as_ref().is_some_and(|x| x.state == AutorouteAttemptState::Routed);
            // the other end only matters if the first one does not route
            let to = if routed(&from) { None } else { attempt(loaded, basic, a.to_item, settings) };
            let class = if routed(&from) || routed(&to) {
                congestion += 1;
                "congestion"
            } else {
                blocked += 1;
                "blocked"
            };
            let in_context: Vec<String> = [1, 10]
                .iter()
                .map(|&pass| {
                    // as the autorouter: from the end that is not connected to a plane already
                    let attempt = |k| fr_engine::pipeline::BatchAutorouter::diagnose_connection(board, settings, k, pass, enhancements);
                    let (mut r, mut ripped, mut inc) = attempt(a.from_item);
                    if matches!(r.state, AutorouteAttemptState::ConnectedToPlane | AutorouteAttemptState::AlreadyConnected) {
                        (r, ripped, inc) = attempt(a.to_item);
                    }
                    format!(
                        "{{\"pass\": {pass}, \"state\": {}, \"details\": {}, \"ripped\": {ripped}, \"unrouted_before_after_tails\": [{}, {}, {}]}}",
                        json_str(&state_name(r.state)),
                        json_str(&r.details),
                        inc[0],
                        inc[1],
                        inc[2]
                    )
                })
                .collect();
            write!(
                s,
                ", \"diagnosis\": {{\"class\": \"{class}\", \"from_alone\": {}, \"to_alone\": {}, \"in_context\": [{}]}}",
                attempt_json(&from),
                attempt_json(&to),
                in_context.join(", ")
            )
            .unwrap();
        }
        s.push('}');
        unrouted.push(s);
    }

    let mut viol = Vec::new();
    for v in &violations {
        let b = v.shape.bounding_box();
        viol.push(format!(
            "    {{\"layer\": {}, \"xy\": [{}, {}], \"clearance_mm\": {}, \"actual_mm\": {}, \"unfixable\": {}, \"category\": {}, \"same_net\": {}, \"first\": {}, \"second\": {}}}",
            u.layer(v.layer),
            u.x((b.ll.x + b.ur.x) as f64 / 2.0),
            u.y((b.ll.y + b.ur.y) as f64 / 2.0),
            num(v.expected_clearance / u.per_mm),
            num(v.actual_clearance / u.per_mm),
            v.is_unfixable(basic),
            json_str(category_name(v.category(basic))),
            basic.item(v.first_item).shares_net(basic.item(v.second_item)),
            item_json(&u, v.first_item),
            item_json(&u, v.second_item),
        ));
    }

    let mut out = String::new();
    writeln!(out, "{{").unwrap();
    writeln!(out, "  \"fastroute\": {},", json_str(env!("CARGO_PKG_VERSION"))).unwrap();
    writeln!(out, "  \"input\": {},", json_str(input)).unwrap();
    writeln!(
        out,
        "  \"time_s\": {{\"load\": {}, \"route\": {}, \"total\": {}, \"diagnose\": {}}},",
        num(timings.load_s),
        num(timings.route_s),
        num(timings.total_s),
        if loaded.is_some() { num(t.elapsed().as_secs_f64()) } else { "null".into() }
    )
    .unwrap();
    writeln!(
        out,
        "  \"stats\": {{\"layers\": {}, \"components\": {}, \"nets\": {}, \"pins\": {}, \"connections\": {}, \"unrouted\": {}, \"unrouted_ignored_classes\": {}, \"violations\": {}, \
         \"violations_unfixable\": {}, \"vias\": {}, \"vias_blind\": {}, \"vias_buried\": {}, \"traces\": {}, \"trace_length_mm\": {}, \
         \"bends_90\": {}, \"bends_45\": {}, \"bends_other\": {}, \"router_score\": {}, \"optimizer_score\": {}}},",
        opt_i(stats.layers.total_count),
        opt_i(stats.components.total_count),
        opt_i(stats.nets.total_count),
        opt_i(stats.items.pin_count),
        opt_i(stats.connections.maximum_count),
        airlines.len(),
        airlines.iter().filter(|a| basic.rules.nets.get(a.net_number).is_some_and(|n| basic.rules.net_classes[n.get_net_class()].is_ignored_by_autorouter)).count(),
        violations.len(),
        violations.iter().filter(|v| v.is_unfixable(basic)).count(),
        opt_i(stats.vias.total_count),
        opt_i(stats.vias.blind_count),
        opt_i(stats.vias.buried_count),
        opt_i(stats.traces.total_count),
        stats.traces.total_length_mm.map(|x| num(x as f64)).unwrap_or_else(|| "null".into()),
        opt_i(stats.bends.ninety_degree_count),
        opt_i(stats.bends.forty_five_degree_count),
        opt_i(stats.bends.other_angle_count),
        num(router_score as f64),
        num(optimizer_score as f64),
    )
    .unwrap();
    if loaded.is_some() {
        writeln!(out, "  \"diagnosis\": {{\"congestion\": {congestion}, \"blocked\": {blocked}, \"ignored\": {ignored}}},").unwrap();
    }
    writeln!(out, "  \"unrouted\": [\n{}\n  ],", unrouted.join(",\n")).unwrap();
    writeln!(out, "  \"clearance_violations\": [\n{}\n  ]", viol.join(",\n")).unwrap();
    writeln!(out, "}}").unwrap();
    std::fs::write(path, out)?;
    if loaded.is_some() {
        log::info!(
            target: "fastroute",
            "diagnosis of {} unrouted connections in {:.2} s: {congestion} routable alone (congestion), {blocked} not routable on the loaded board (blocked), {ignored} in ignored net classes",
            airlines.len(),
            t.elapsed().as_secs_f64()
        );
    }
    Ok(())
}
