//! fastroute: length matching after routing.
//!
//! A tuning group is a set of nets that should have (about) the same routed length, e.g. the
//! data lines of a memory bus. The target is the longest net of the group (or a given
//! length); every net shorter than `target - tolerance` gets meanders: a straight trace
//! segment is replaced by a serpentine with the same end points,
//!
//! ```text
//!  ──┐  ┌──┐  ┌──        amplitude a, leg pitch s:
//!    │  │  │  │          every bump adds 2·a of length
//!    └──┘  └──┘
//! ```
//!
//! Segments are tried longest first, bumps on either side of the segment or alternating,
//! with decreasing amplitudes. Every change is made on a clone of the board and kept only if
//! the new trace has no clearance violation (other nets, keepouts, board edge), so the board
//! stays DRC-clean. Lengths are trace lengths (vias are not counted: the DSN has no layer
//! thicknesses).

use crate::board::{ItemKey, RoutingBoard};
use crate::drc::clearance_violation::clearance_violation_count;
use crate::ids::{FixedState, LayerNo, NetNo};
use fr_geom::{FloatPoint, IntPoint, Line, Point, Polyline};

/// Extra half width of the clearance check of a meander, micrometres.
const TUNING_SAFETY_MARGIN_UM: f64 = 5.0;

/// One group of nets to match.
#[derive(Clone, Debug)]
pub struct TuneGroup {
    pub name: String,
    /// Net names; `*` matches any text.
    pub nets: Vec<String>,
    /// Allowed deviation below the target, mm.
    pub tolerance_mm: f64,
    /// Target length, mm (`None`: the longest net of the group).
    pub target_mm: Option<f64>,
}

/// Result for one net.
#[derive(Clone, Debug)]
pub struct NetTuning {
    pub net: String,
    pub before_mm: f64,
    pub after_mm: f64,
}

/// Result for one group.
#[derive(Clone, Debug)]
pub struct GroupTuning {
    pub name: String,
    pub target_mm: f64,
    pub tolerance_mm: f64,
    pub nets: Vec<NetTuning>,
}

impl GroupTuning {
    /// Nets still shorter than `target - tolerance`.
    pub fn short_nets(&self) -> usize {
        self.nets.iter().filter(|n| n.after_mm < self.target_mm - self.tolerance_mm - 1e-6).count()
    }
}

fn glob_match(pattern: &str, text: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == text;
    }
    let mut rest = text;
    for (i, part) in parts.iter().enumerate() {
        if i == 0 {
            if !rest.starts_with(part) {
                return false;
            }
            rest = &rest[part.len()..];
        } else if i == parts.len() - 1 {
            return rest.ends_with(part);
        } else if let Some(p) = rest.find(part) {
            rest = &rest[p + part.len()..];
        } else {
            return false;
        }
    }
    true
}

fn units_per_mm(board: &RoutingBoard) -> f64 {
    board.communication.resolution.max(1) as f64 * 1000.0
}

fn net_traces(board: &RoutingBoard, net: NetNo) -> Vec<ItemKey> {
    board.get_connectable_items(net).into_iter().filter(|&k| board.item(k).is_trace()).collect()
}

/// Height of each copper layer from the top of the board, mm (index = layer number); empty:
/// vias do not count. KiCad adds the stackup distance between the layers a via connects to
/// the net length; set from the board's stackup (`--layer-heights`).
static LAYER_HEIGHTS_MM: std::sync::RwLock<Vec<f64>> = std::sync::RwLock::new(Vec::new());

/// Sets the copper layer heights used for via lengths (see [`net_length`]).
pub fn set_layer_heights_mm(heights: Vec<f64>) {
    *LAYER_HEIGHTS_MM.write().unwrap() = heights;
}

/// Routed length of a net in board units: its traces plus, for every via, the height between
/// the outermost layers its traces use there (as KiCad measures it).
pub fn net_length(board: &RoutingBoard, net: NetNo) -> f64 {
    let traces = net_traces(board, net);
    let mut len: f64 = traces.iter().map(|&k| board.item(k).as_trace().map(|t| t.length()).unwrap_or(0.0)).sum();
    let heights = LAYER_HEIGHTS_MM.read().unwrap();
    if heights.is_empty() {
        return len;
    }
    let upm = units_per_mm(board);
    let ends: Vec<(LayerNo, Point, Point)> = traces
        .iter()
        .filter_map(|&k| board.item(k).as_trace().map(|t| (t.layer(), t.first_corner(), t.last_corner())))
        .collect();
    for k in board.get_connectable_items(net) {
        let item = board.item(k);
        if !item.is_via() {
            continue;
        }
        let c = item.center(board);
        let layers: Vec<usize> = ends.iter().filter(|(_, a, b)| *a == c || *b == c).map(|(l, _, _)| *l as usize).collect();
        if let (Some(&lo), Some(&hi)) = (layers.iter().min(), layers.iter().max()) {
            if hi < heights.len() {
                len += (heights[hi] - heights[lo]) * upm;
            }
        }
    }
    len
}

/// Tunes all groups; returns what was done.
pub fn tune_lengths(board: &mut RoutingBoard, groups: &[TuneGroup]) -> Vec<GroupTuning> {
    let upm = units_per_mm(board);
    let mut out = Vec::new();
    for g in groups {
        let mut nets: Vec<(NetNo, String)> = Vec::new();
        for n in 1..=board.rules.nets.max_net_number() {
            if let Some(net) = board.rules.nets.get(n) {
                if g.nets.iter().any(|p| glob_match(p, &net.name)) {
                    nets.push((n, net.name.clone()));
                }
            }
        }
        if nets.is_empty() {
            log::warn!(target: "fr_engine::pipeline", "length tuning: group '{}' matches no net", g.name);
            continue;
        }
        let before: Vec<f64> = nets.iter().map(|&(n, _)| net_length(board, n)).collect();
        let longest = before.iter().cloned().fold(0.0, f64::max);
        let target = g.target_mm.map(|t| t * upm).unwrap_or(longest);
        let tol = g.tolerance_mm * upm;
        let mut result = GroupTuning { name: g.name.clone(), target_mm: target / upm, tolerance_mm: g.tolerance_mm, nets: Vec::new() };
        // two rounds: meanders of one net can free or block space for another; nets furthest
        // from the target first (they need the most room)
        let mut order: Vec<usize> = (0..nets.len()).collect();
        order.sort_by(|&a, &b| before[a].partial_cmp(&before[b]).unwrap_or(std::cmp::Ordering::Equal));
        for _round in 0..2 {
            for &i in &order {
                let now = net_length(board, nets[i].0);
                if now > 0.0 && now < target - tol {
                    // aim at the middle of the window so small rounding does not leave it short
                    tune_net(board, nets[i].0, target - tol / 2.0 - now);
                }
            }
        }
        for (i, (net, name)) in nets.iter().enumerate() {
            result.nets.push(NetTuning { net: name.clone(), before_mm: before[i] / upm, after_mm: net_length(board, *net) / upm });
        }
        out.push(result);
    }
    out
}

/// How [`tune_net_with`] picks its segments.
pub struct TuneOptions<'a> {
    /// Rank of a straight segment (`key`, index, end points): higher first, `None` = leave it
    /// alone. The default ranks by length.
    pub rank: &'a dyn Fn(&RoutingBoard, ItemKey, usize, &FloatPoint, &FloatPoint) -> Option<f64>,
    /// Largest meander amplitude, mm (default 4).
    pub max_amp_mm: f64,
}

/// Adds about `extra` board units of length to `net` with meanders; returns the added length.
fn tune_net(board: &mut RoutingBoard, net: NetNo, extra: f64) -> f64 {
    tune_net_with(board, net, extra, &TuneOptions { rank: &|_, _, _, a, b| Some(a.distance(b)), max_amp_mm: 4.0 })
}

/// [`tune_net`] with the segment choice and amplitude of `opts` (the diff pair skew matching
/// keeps the meanders off the coupled runs).
pub fn tune_net_with(board: &mut RoutingBoard, net: NetNo, extra: f64, opts: &TuneOptions) -> f64 {
    let upm = units_per_mm(board);
    let mut need = extra;
    let mut added = 0.0;
    // amplitudes to try, mm (large bumps first: fewer corners)
    const AMPLITUDES_MM: [f64; 8] = [4.0, 3.0, 2.0, 1.4, 1.0, 0.7, 0.45, 0.3];
    let mut failed_segments: std::collections::HashSet<(i32, i32)> = std::collections::HashSet::new();
    'outer: while need > 0.02 * upm {
        // best ranked straight segments of the net's traces first
        let mut segs: Vec<(f64, ItemKey, usize)> = Vec::new();
        for k in net_traces(board, net) {
            let Some(t) = board.item(k).as_trace() else { continue };
            let corners = t.polyline().corners();
            for i in 0..corners.len().saturating_sub(1) {
                let a = corners[i].to_float();
                let b = corners[i + 1].to_float();
                if failed_segments.contains(&(board.item(k).id().0, i as i32)) {
                    continue;
                }
                if let Some(r) = (opts.rank)(board, k, i, &a, &b) {
                    segs.push((r, k, i));
                }
            }
        }
        segs.sort_by(|x, y| y.0.partial_cmp(&x.0).unwrap_or(std::cmp::Ordering::Equal));
        for (_, key, seg) in segs {
            let id = board.item(key).id().0;
            let half_width = board.item(key).as_trace().map(|t| t.half_width()).unwrap_or(0) as f64;
            let clearance = board.rules.clearance_matrix.get_value(board.item(key).clearance_class(), board.item(key).clearance_class(), board.item(key).as_trace().map(|t| t.layer()).unwrap_or(0), false) as f64;
            // centre distance of neighbouring legs: 3 widths (the usual crosstalk rule) where it
            // fits, else one clearance between the copper
            let pitches = [(6.0 * half_width).max(2.0 * half_width + clearance), 2.0 * half_width + clearance];
            for pitch in pitches {
                for amp_mm in AMPLITUDES_MM {
                    if amp_mm > opts.max_amp_mm + 1e-9 {
                        continue;
                    }
                    let amp = amp_mm * upm;
                    if amp < 2.0 * half_width + clearance {
                        continue;
                    }
                    for style in [Side::Left, Side::Right, Side::Alternate] {
                        if let Some((next, got)) = try_meander(board, key, seg, need, amp, pitch, style) {
                            if log::log_enabled!(target: "fr_engine::pipeline::diag", log::Level::Debug) {
                                let real = net_length(&next, net) - net_length(board, net);
                                log::debug!(target: "fr_engine::pipeline::diag", "meander net {net}: computed {:.3} mm, real {:.3} mm (amp {:.2}, pitch {:.2}, {:?}, traces {} -> {})", got / upm, real / upm, amp / upm, pitch / upm, style, net_traces(board, net).len(), net_traces(&next, net).len());
                            }
                            *board = next;
                            need -= got;
                            added += got;
                            continue 'outer;
                        }
                    }
                }
            }
            failed_segments.insert((id, seg as i32));
        }
        break;
    }
    added
}

#[derive(Clone, Copy, Debug)]
enum Side {
    Left,
    Right,
    Alternate,
}

/// Replaces segment `seg` of trace `key` by a serpentine adding up to `need`; returns the new
/// board and the added length if the result has no new clearance violation.
fn try_meander(board: &RoutingBoard, key: ItemKey, seg: usize, need: f64, max_amp: f64, pitch: f64, side: Side) -> Option<(RoutingBoard, f64)> {
    let item = board.item(key);
    let t = item.as_trace()?;
    // locked tracks (KiCad "fix") and footprint copper stay as they are; unlocked routed
    // tracks of an imported board (DSN "route" = USER_FIXED) may be tuned
    if item.fixed_state() >= FixedState::SystemFixed || item.component_no() > 0 {
        return None;
    }
    let (polyline, added) = meander_polyline(t.polyline(), seg, need, max_amp, pitch, side)?;

    let mut next = board.clone();
    let (layer, half_width, nets, class, fixed) =
        (t.layer(), t.half_width(), item.net_numbers().to_vec(), item.clearance_class(), item.fixed_state());
    let old = next.get_item(item.id())?;
    if next.item(old).is_user_fixed() {
        next.items.get_mut(old).set_fixed_state(FixedState::Unfixed);
    }
    next.remove_item(old);
    if next.get_item(item.id()).is_some() {
        return None; // not removed: never leave the old and the new trace on top of each other
    }
    // The check uses a slightly wider trace: KiCad measures the exact 45-degree geometry and
    // finds micrometre violations the integer checks here miss (the router keeps the same
    // safety margin when it routes).
    let mut probe = next.clone();
    let margin = (TUNING_SAFETY_MARGIN_UM * next.communication.resolution.max(1) as f64).round() as i32;
    let probe_key = probe.insert_trace_without_cleaning(polyline.clone(), layer, half_width + margin, &nets, class, fixed)?;
    if clearance_violation_count(&probe, probe_key) > 0 {
        return None;
    }
    next.insert_trace_without_cleaning(polyline, layer, half_width, &nets, class, fixed)?;
    Some((next, added))
}

/// The serpentine replacing segment `seg` of `polyline` (between corners `seg` and `seg + 1`),
/// adding up to `need`; returns the new polyline and the added length.
fn meander_polyline(polyline: &Polyline, seg: usize, need: f64, max_amp: f64, pitch: f64, side: Side) -> Option<(Polyline, f64)> {
    let corners = polyline.corners();
    let (a, b) = (&corners[seg], &corners[seg + 1]);
    let (Point::Int(pa), Point::Int(pb)) = (a, b) else { return None };
    let (dx, dy) = ((pb.x - pa.x) as i64, (pb.y - pa.y) as i64);
    // only 45-degree directions: integer unit step e with |e| = 1 or sqrt(2)
    let (ex, ey) = (dx.signum(), dy.signum());
    if !(dx == 0 || dy == 0 || dx.abs() == dy.abs()) {
        return None;
    }
    let steps = dx.abs().max(dy.abs()); // segment length in steps of e
    let step_len = if ex != 0 && ey != 0 { std::f64::consts::SQRT_2 } else { 1.0 };
    let (nx, ny) = (-ey, ex);
    let pitch_steps = (pitch / step_len).ceil() as i64;
    // keep the bumps half a pitch away from the segment's corners
    let margin = (pitch_steps + 1) / 2;
    // bumps that fit: 2*margin + (2k - 1) * pitch <= steps
    let k_cap = (steps - 2 * margin + pitch_steps) / (2 * pitch_steps);
    if k_cap <= 0 {
        return None;
    }
    let per_bump_max = 2.0 * max_amp;
    let k = ((need / per_bump_max).ceil() as i64).clamp(1, k_cap);
    let amp = (need / (2.0 * k as f64)).min(max_amp);
    let amp_steps = (amp / step_len).round() as i64;
    if amp_steps <= 0 {
        return None;
    }
    // centre the bumps on the segment
    let used = (2 * k - 1) * pitch_steps;
    let start = (steps - used) / 2;
    // the meander's own corners, from corner seg to corner seg + 1 (all IntPoints)
    let mut pts: Vec<Point> = vec![a.clone()];
    let at = |s: i64, o: i64| Point::Int(IntPoint::new((pa.x as i64 + ex * s + nx * o) as i32, (pa.y as i64 + ey * s + ny * o) as i32));
    let mut s = start;
    for i in 0..k {
        let sign = match side {
            Side::Left => 1,
            Side::Right => -1,
            Side::Alternate => {
                if i % 2 == 0 {
                    1
                } else {
                    -1
                }
            }
        };
        let o = sign * amp_steps;
        pts.push(at(s, 0));
        pts.push(at(s, o));
        pts.push(at(s + pitch_steps, o));
        pts.push(at(s + pitch_steps, 0));
        s += 2 * pitch_steps;
    }
    pts.push(b.clone());
    let added = 2.0 * (k * amp_steps) as f64 * step_len;
    // Splice lines, not corners: corner i is lines[i] x lines[i + 1], so the segment is
    // lines[seg + 1]. The trace's other corners may be RationalPoints (intersections of
    // any-angle lines); rebuilding the trace from its corners drew lines through them, and
    // Line::intersection_approx, which requires IntPoint ends as in Freerouting, panicked.
    let mut lines: Vec<Line> = polyline.lines[..=seg].to_vec();
    for w in pts.windows(2) {
        if w[0] != w[1] {
            lines.push(Line::new(w[0].clone(), w[1].clone()));
        }
    }
    lines.extend_from_slice(&polyline.lines[seg + 2..]);
    Some((Polyline::from_lines(lines), added))
}

#[cfg(test)]
mod tests {
    use super::{glob_match, meander_polyline, Side};
    use fr_geom::{Line, Point, Polyline};

    /// A trace whose segment 0 (corners (0,0) -> (1000,0)) can take a meander, and whose
    /// corner 3 is the intersection of a slope-2 and a slope-1/3 line: x = 2370.6, rational.
    fn trace_with_rational_corner() -> Polyline {
        Polyline::from_lines(vec![
            Line::new_ints(0, -10, 0, 10),       // start cap, x = 0
            Line::new_ints(0, 0, 1000, 0),       // y = 0
            Line::new_ints(1000, 0, 1000, 10),   // x = 1000
            Line::new_ints(1000, 50, 1001, 52),  // slope 2 through (1000, 50)
            Line::new_ints(0, 2001, 3, 2002),    // slope 1/3
            Line::new_ints(3000, 0, 3000, 1),    // end cap, x = 3000
        ])
    }

    #[test]
    fn meander_on_an_all_integer_trace_is_unchanged_by_the_splice() {
        // what try_meander built before the fix: the polyline through every corner
        let pl = Polyline::from_lines(vec![
            Line::new_ints(0, -10, 0, 10),
            Line::new_ints(0, 0, 1000, 0),
            Line::new_ints(1000, 0, 1000, 10),
            Line::new_ints(0, 600, 10, 600),
            Line::new_ints(1500, 0, 1500, 1),
        ]);
        for side in [Side::Left, Side::Right, Side::Alternate] {
            let (m, added) = meander_polyline(&pl, 0, 350.0, 100.0, 100.0, side).expect("a meander fits");
            let c = pl.corners();
            let (mut pts, _) = (c[..1].to_vec(), ());
            let mc = m.corners();
            // the old construction: original corners up to seg, the meander's, the rest
            pts.extend(mc[1..mc.len() - (c.len() - 2)].iter().cloned());
            pts.extend(c[1..].iter().cloned());
            assert_eq!(Polyline::from_points(&pts).corners(), mc, "{side:?}");
            assert!(added > 0.0);
        }
    }

    #[test]
    fn meander_beside_a_rational_corner() {
        let pl = trace_with_rational_corner();
        let corners = pl.corners();
        assert!(matches!(corners[0], Point::Int(_)) && matches!(corners[1], Point::Int(_)));
        assert!(!matches!(corners[3], Point::Int(_)), "the fixture needs a rational corner");
        let (m, added) = meander_polyline(&pl, 0, 200.0, 100.0, 100.0, Side::Left).expect("a meander fits");
        // inserting a trace builds its offset shapes: this panicked with
        // "ClassCastException: RationalPoint is not an IntPoint" (fr-geom point.rs)
        let _ = m.offset_shapes(5);
        assert_eq!(added, 200.0);
        // the rest of the trace is unchanged: same ends, same rational corner
        let mc = m.corners();
        assert_eq!(mc.first(), corners.first());
        assert_eq!(mc.last(), corners.last());
        assert!(mc.contains(&corners[3]));
    }

    #[test]
    fn globs() {
        assert!(glob_match("/SD_D*", "/SD_D12"));
        assert!(glob_match("*CLK", "/SD_CLK"));
        assert!(glob_match("/SD_*_N", "/SD_D0_N"));
        assert!(!glob_match("/SD_D*", "/SD_A1"));
        assert!(glob_match("/X", "/X"));
    }
}
