//! fastroute extension: exact clearance for the clearance-violation report.
//!
//! The board's tile shapes approximate round geometry by bounding octagons (a via's circle, a trace's
//! round end). An octagon reaches up to 8.2 % past the circle it bounds, so the clearance measured
//! between two octagons is smaller than the true one, by about 0.06 mm for a 1.5 mm power trace. The
//! clearance check reported copper KiCad's exact DRC passes as "pre-existing violations". This module
//! measures the true geometry: a via is a circle, a trace segment is a capsule (segment + half width),
//! a pad keeps its own convex shape (a circle when its padstack is one). Routing keeps the octagons,
//! which only err on the safe side.

use fr_geom::{ConvexShape, FloatPoint, TileShape};

use crate::board::basic_board::BasicBoard;
use crate::board::item::ItemKind;
use crate::board::ItemKey;

/// One exact copper primitive.
#[derive(Clone, Debug)]
pub enum Prim {
    Circle(FloatPoint, f64),
    Capsule(FloatPoint, FloatPoint, f64),
    Poly(Vec<FloatPoint>),
}

fn sub(a: &FloatPoint, b: &FloatPoint) -> (f64, f64) {
    (a.x - b.x, a.y - b.y)
}

/// Distance from point `p` to segment `a`-`b`.
pub fn point_segment(p: &FloatPoint, a: &FloatPoint, b: &FloatPoint) -> f64 {
    let (dx, dy) = sub(b, a);
    let l2 = dx * dx + dy * dy;
    let t = if l2 <= 0.0 { 0.0 } else { (((p.x - a.x) * dx + (p.y - a.y) * dy) / l2).clamp(0.0, 1.0) };
    p.distance(&FloatPoint::new(a.x + t * dx, a.y + t * dy))
}

fn cross(o: &FloatPoint, a: &FloatPoint, b: &FloatPoint) -> f64 {
    (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x)
}

fn segments_cross(a: &FloatPoint, b: &FloatPoint, c: &FloatPoint, d: &FloatPoint) -> bool {
    let (d1, d2) = (cross(c, d, a), cross(c, d, b));
    let (d3, d4) = (cross(a, b, c), cross(a, b, d));
    ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0)) && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
}

/// Distance between segments `a`-`b` and `c`-`d` (0 if they cross).
pub fn segment_segment(a: &FloatPoint, b: &FloatPoint, c: &FloatPoint, d: &FloatPoint) -> f64 {
    if segments_cross(a, b, c, d) {
        return 0.0;
    }
    point_segment(a, c, d).min(point_segment(b, c, d)).min(point_segment(c, a, b)).min(point_segment(d, a, b))
}

/// True if `p` lies inside (or on) the convex polygon `v` (either winding).
fn inside(p: &FloatPoint, v: &[FloatPoint]) -> bool {
    if v.len() < 3 {
        return false;
    }
    let mut sign = 0.0f64;
    for i in 0..v.len() {
        let c = cross(&v[i], &v[(i + 1) % v.len()], p);
        if c.abs() < 1e-9 {
            continue;
        }
        if sign == 0.0 {
            sign = c.signum();
        } else if c.signum() != sign {
            return false;
        }
    }
    true
}

/// Distance from segment `a`-`b` (a point when a == b) to the convex polygon `v` (0 if they touch).
fn segment_poly(a: &FloatPoint, b: &FloatPoint, v: &[FloatPoint]) -> f64 {
    if inside(a, v) || inside(b, v) {
        return 0.0;
    }
    (0..v.len()).map(|i| segment_segment(a, b, &v[i], &v[(i + 1) % v.len()])).fold(f64::MAX, f64::min)
}

/// The exact gap between two primitives (negative when they overlap); None for polygon pairs, which
/// the octagon measurement already handles exactly (a polygon is not approximated).
pub fn gap(p: &Prim, q: &Prim) -> Option<f64> {
    use Prim::*;
    Some(match (p, q) {
        (Circle(c1, r1), Circle(c2, r2)) => c1.distance(c2) - r1 - r2,
        (Circle(c, r), Capsule(a, b, h)) | (Capsule(a, b, h), Circle(c, r)) => point_segment(c, a, b) - r - h,
        (Capsule(a, b, h1), Capsule(c, d, h2)) => segment_segment(a, b, c, d) - h1 - h2,
        (Circle(c, r), Poly(v)) | (Poly(v), Circle(c, r)) => segment_poly(c, c, v) - r,
        (Capsule(a, b, h), Poly(v)) | (Poly(v), Capsule(a, b, h)) => segment_poly(a, b, v) - h,
        (Poly(_), Poly(_)) => return None,
    })
}

fn poly_of(t: &TileShape) -> Vec<FloatPoint> {
    t.corner_approx_arr()
}

/// The exact primitive of tile shape `index` of item `key`, if it has one.
pub fn prim(board: &BasicBoard, key: ItemKey, index: i32) -> Option<Prim> {
    let item = board.item(key);
    match &item.kind {
        ItemKind::Trace(t) => {
            let a = t.polyline.corner_approx(index);
            let b = t.polyline.corner_approx(index + 1);
            Some(Prim::Capsule(a, b, t.half_width as f64))
        }
        ItemKind::Via(_) | ItemKind::Pin(_) => {
            let padstack = item.padstack(board)?;
            let layer = board.shape_layer(key, index);
            let center = item.center(board).to_float();
            match padstack.get_shape(layer) {
                // a round pad or a via: its circle (rotation and mirroring leave a centred circle as is)
                Some(ConvexShape::Circle(c)) if c.center.x == 0 && c.center.y == 0 => {
                    Some(Prim::Circle(center, c.radius as f64))
                }
                _ => board.tile_shape(key, index).map(|t| Prim::Poly(poly_of(&t))),
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: f64, y: f64) -> FloatPoint {
        FloatPoint::new(x, y)
    }

    #[test]
    fn circles_and_capsules() {
        // two vias of radius 225 at 600 pitch: 150 apart
        assert!((gap(&Prim::Circle(p(0.0, 0.0), 225.0), &Prim::Circle(p(600.0, 0.0), 225.0)).unwrap() - 150.0).abs() < 1e-9);
        // a via beside a trace (half width 750) running 1125 away from its centre: 150
        let g = gap(&Prim::Circle(p(0.0, 0.0), 225.0), &Prim::Capsule(p(-5000.0, 1125.0), p(5000.0, 1125.0), 750.0)).unwrap();
        assert!((g - 150.0).abs() < 1e-9, "{g}");
        // a trace's round END: the via sits off the end, 1125 from the end point
        let g = gap(&Prim::Circle(p(0.0, 0.0), 225.0), &Prim::Capsule(p(1125.0, 0.0), p(5000.0, 0.0), 750.0)).unwrap();
        assert!((g - 150.0).abs() < 1e-9, "{g}");
        // crossing traces overlap
        assert!(gap(&Prim::Capsule(p(-10.0, 0.0), p(10.0, 0.0), 1.0), &Prim::Capsule(p(0.0, -10.0), p(0.0, 10.0), 1.0)).unwrap() < 0.0);
    }

    #[test]
    fn polygons() {
        let sq = Prim::Poly(vec![p(0.0, 0.0), p(100.0, 0.0), p(100.0, 100.0), p(0.0, 100.0)]);
        assert!((gap(&sq, &Prim::Circle(p(250.0, 50.0), 50.0)).unwrap() - 100.0).abs() < 1e-9);
        assert!(gap(&sq, &Prim::Circle(p(50.0, 50.0), 10.0)).unwrap() < 0.0);
        assert!((gap(&sq, &Prim::Capsule(p(-200.0, 300.0), p(300.0, 300.0), 50.0)).unwrap() - 150.0).abs() < 1e-9);
        assert!(gap(&sq, &sq).is_none());
    }
}
