//! Port of `drc/ClearanceViolation.java` and of `Item.clearanceViolations()` (`Item.java`) with
//! its `Via` override (reference source version: clearance tolerance, board outline exemptions,
//! escape via filtering).

use fr_geom::{ConvexShape, TileShape};
use fr_jcompat::math_round;

use crate::board::{BasicBoard, BoardOutline, ItemKey, TreeObject, DEFAULT_TREE};
use crate::ids::LayerNo;
use crate::structure::Unit;

/// Java `ClearanceViolation.Category`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Category {
    PinToPin,
    PinToOutlineOrKeepout,
    FixedRoute,
    OtherUnfixable,
    PotentiallyFixable,
}

/// Java `ClearanceViolation`: a clearance violation between two items. `first_item` is the item
/// whose `clearanceViolations()` reported it.
#[derive(Clone, Debug)]
pub struct ClearanceViolation {
    pub first_item: ItemKey,
    pub second_item: ItemKey,
    /// The intersection of the two (clearance enlarged) shapes.
    pub shape: TileShape,
    pub layer: LayerNo,
    pub expected_clearance: f64,
    pub actual_clearance: f64,
}

impl ClearanceViolation {
    /// Java `isUnfixable()`: neither participant is routable.
    pub fn is_unfixable(&self, board: &BasicBoard) -> bool {
        !board.item(self.first_item).is_routable() && !board.item(self.second_item).is_routable()
    }

    /// Java `getCategory()`.
    pub fn category(&self, board: &BasicBoard) -> Category {
        if !self.is_unfixable(board) {
            return Category::PotentiallyFixable;
        }
        let first = board.item(self.first_item);
        let second = board.item(self.second_item);
        let first_is_pin = first.is_pin();
        let second_is_pin = second.is_pin();
        if first_is_pin && second_is_pin {
            return Category::PinToPin;
        }
        let outline_or_keepout =
            |it: &crate::board::Item| it.is_board_outline() || it.is_component_outline() || it.is_obstacle_area();
        if (first_is_pin && outline_or_keepout(second)) || (second_is_pin && outline_or_keepout(first)) {
            return Category::PinToOutlineOrKeepout;
        }
        let is_route = |it: &crate::board::Item| it.is_trace() || it.is_via();
        if is_route(first) || is_route(second) {
            return Category::FixedRoute;
        }
        Category::OtherUnfixable
    }
}

/// Java `Item.clearanceViolations()` (with the `Via` override). Does not update the item's
/// `smallest_clearance` (see [`clearance_violations_updating`]).
pub fn clearance_violations(board: &BasicBoard, key: ItemKey) -> Vec<ClearanceViolation> {
    let mut smallest = board.item(key).smallest_clearance;
    clearance_violations_tracking(board, key, &mut smallest)
}

/// Like [`clearance_violations`], and stores the Java side effect on `Item.smallestClearance`.
pub fn clearance_violations_updating(board: &mut BasicBoard, key: ItemKey) -> Vec<ClearanceViolation> {
    let mut smallest = board.item(key).smallest_clearance;
    let result = clearance_violations_tracking(board, key, &mut smallest);
    board.item_mut(key).smallest_clearance = smallest;
    result
}

/// Java `Item.clearanceViolationCount()`.
pub fn clearance_violation_count(board: &BasicBoard, key: ItemKey) -> usize {
    clearance_violations(board, key).len()
}

/// [`clearance_violations`] with the `smallestClearance` side effect applied to `smallest`.
pub fn clearance_violations_tracking(board: &BasicBoard, key: ItemKey, smallest: &mut f64) -> Vec<ClearanceViolation> {
    let raw = item_clearance_violations(board, key, smallest);
    // Via.clearanceViolations(): escape vias may overlap same-net items on their SMD layer.
    let item = board.item(key);
    if let Some(via) = item.as_via() {
        if via.is_escape_via && via.escape_via_smd_layer >= 0 {
            return raw
                .into_iter()
                .filter(|v| {
                    if v.layer != via.escape_via_smd_layer {
                        return true;
                    }
                    // firstItem is always this item, so the other one is secondItem
                    let other = if v.first_item == key {
                        Some(v.second_item)
                    } else if v.second_item == key {
                        Some(v.first_item)
                    } else {
                        None
                    };
                    !matches!(other, Some(o) if board.item(o).shares_net(item))
                })
                .collect();
        }
    }
    raw
}

/// Java `Item.clearanceViolations()` (the base implementation).
fn item_clearance_violations(board: &BasicBoard, key: ItemKey, smallest_clearance: &mut f64) -> Vec<ClearanceViolation> {
    let mut result = Vec::new();
    let this = board.item(key);
    let cl_class = this.clearance_class();
    let compensation_used = board.is_clearance_compensation_used();
    for i in 0..board.tile_shape_count(key) {
        let Some(current_tile_shape) = board.tile_shape(key, i) else {
            // Java would throw a NullPointerException in the tree query.
            log::warn!("Item.clearanceViolations: tile shape {i} of item {} is null", this.id().0);
            continue;
        };
        let layer = board.shape_layer(key, i);
        let entries = board.overlapping_tree_entries_with_clearance(
            DEFAULT_TREE,
            &ConvexShape::Tile(current_tile_shape.clone()),
            layer,
            &[],
            cl_class,
        );
        for entry in entries {
            let TreeObject::Item { key: other_key, .. } = entry.object else {
                continue;
            };
            if other_key == key {
                continue;
            }
            let other = board.item(other_key);
            let mut is_obstacle = other.is_obstacle(other_key, this, key, board);
            if is_obstacle && this.is_trace() && other.is_trace() {
                // Look, if both traces are connected to the same tie pin. In this case they are
                // allowed to overlap without sharing a net.
                let mut contacts = board.trace_normal_contacts_at(key, &this.first_corner(), true);
                let mut contact_found = contacts.contains(other.id());
                if !contact_found {
                    contacts = board.trace_normal_contacts_at(key, &this.last_corner(), true);
                    contact_found = contacts.contains(other.id());
                }
                if contact_found {
                    for c in contacts.iter() {
                        let contact = board.item(c);
                        if contact.is_pin() && contact.shares_net(this) && contact.shares_net(other) {
                            is_obstacle = false;
                            break;
                        }
                    }
                }
            }

            if is_obstacle {
                if this.is_board_outline() && other.is_trace() {
                    if !board.outline_blocks_nets(other.net_numbers()) {
                        is_obstacle = false;
                    }
                } else if this.is_trace() && other.is_board_outline() {
                    if !board.outline_blocks_nets(this.net_numbers()) {
                        is_obstacle = false;
                    }
                } else if (this.is_board_outline() && other.is_pin()) || (this.is_pin() && other.is_board_outline()) {
                    let outline = if this.is_board_outline() {
                        this.as_board_outline().unwrap()
                    } else {
                        other.as_board_outline().unwrap()
                    };
                    // Use the actual pad tile shape (not just its center) to determine containment.
                    let pin_tile_shape = if this.is_pin() {
                        Some(current_tile_shape.clone())
                    } else {
                        board.tile_shape(other_key, entry.shape_index)
                    };
                    if let Some(s) = pin_tile_shape {
                        if outline_contains_tile_shape(outline, &s) {
                            is_obstacle = false;
                        }
                    }
                }
            }

            if !is_obstacle {
                continue;
            }
            // fastroute extension: two pads of one component are the part's own geometry, which no router
            // moves (a USB-C receptacle's GND and VBUS pads sit 0.10 mm apart by design)
            if board.rules.skip_same_component_pin_pairs && this.is_pin() && other.is_pin() && this.component_no > 0
                && this.component_no == other.component_no {
                continue;
            }
            // fastroute extension: two vias of ONE net are held to the fab's drill-to-drill rule, not to the
            // copper clearance between different nets (which reported every via pair of a decoupling fanout
            // at 0.45-0.6 mm pitch). Only when router.same_net_via_hole_to_hole_um is set.
            if let (Some(hth), true, true) = (board.rules.get_same_net_via_hole_to_hole(), this.is_via(), other.is_via()) {
                if other.shares_net(this) {
                    let (pa, pb) = (this.padstack(board), other.padstack(board));
                    if let (Some(pa), Some(pb)) = (pa, pb) {
                        let (ca, cb) = (this.center(board), other.center(board));
                        let gap = ca.to_float().distance(&cb.to_float()) - pa.get_drill_radius() - pb.get_drill_radius();
                        if gap + 1e-6 >= hth {
                            continue;
                        }
                        if let Some(shape2) = board.tile_shape(other_key, entry.shape_index) {
                            let intersection = current_tile_shape.intersection(&shape2);
                            if *smallest_clearance < 0.0 || gap < *smallest_clearance {
                                *smallest_clearance = gap;
                            }
                            result.push(ClearanceViolation {
                                first_item: key,
                                second_item: other_key,
                                shape: intersection,
                                layer,
                                expected_clearance: hth,
                                actual_clearance: gap,
                            });
                        }
                        continue;
                    }
                }
            }
            // the two shapes the clearance is calculated between
            let shape1 = &current_tile_shape;
            let Some(shape2) = board.tile_shape(other_key, entry.shape_index) else {
                log::warn!(
                    "Item.clearanceViolations: unexpected null shape (shape2 is null) between item1 id={} and item2 id={}",
                    this.id().0,
                    other.id().0
                );
                continue;
            };
            let minimum_clearance = board.rules.clearance_matrix.get_value(other.clearance_class(), cl_class, layer, false) as f64;
            let (cl_comp1, cl_comp2) = if compensation_used {
                (
                    board.clearance_compensation_value(DEFAULT_TREE, cl_class, layer),
                    board.clearance_compensation_value(DEFAULT_TREE, other.clearance_class(), layer),
                )
            } else {
                let c1 = math_round(0.5 * minimum_clearance) as i32;
                let c2 = math_round(minimum_clearance - c1 as f64) as i32;
                (c1, c2)
            };
            let enlarged1 = if cl_comp1 > 0 { shape1.enlarge(cl_comp1 as f64) } else { shape1.clone() };
            let enlarged2 = if cl_comp2 > 0 { shape2.enlarge(cl_comp2 as f64) } else { shape2.clone() };
            let intersection = enlarged1.intersection(&enlarged2);
            if intersection.dimension() != 2 {
                continue;
            }
            let mut actual_clearance = clearance_between_two_shapes(shape1, &shape2, minimum_clearance, cl_comp1, cl_comp2);
            // fastroute extension: the tile shapes bound round copper by octagons, which read too close;
            // re-measure a reported shortfall on the true geometry (drc::exact) before calling it one
            if board.rules.exact_clearance_check && actual_clearance < minimum_clearance {
                if let (Some(pa), Some(pb)) = (super::exact::prim(board, key, i), super::exact::prim(board, other_key, entry.shape_index)) {
                    if let Some(g) = super::exact::gap(&pa, &pb) {
                        actual_clearance = actual_clearance.max(g);
                    }
                }
            }
            // fastroute extension: inside an exempt area of one item's net class the class-pair clearance
            // does not apply (KiCad's rule-area scoping, e.g. usb_spacing inside the USB pin field)
            let mut minimum_clearance = minimum_clearance;
            if !board.rules.clearance_exempt_areas.is_empty() {
                let (b1, b2) = (shape1.bounding_box(), shape2.bounding_box());
                let classes: Vec<String> = [this, other].iter().flat_map(|it| it.net_numbers().iter()
                    .map(|&n| board.rules.net_class_of(n).get_name().to_string())).collect();
                for (cls, bx, base) in &board.rules.clearance_exempt_areas {
                    if classes.iter().any(|c| c == cls) && (b1.intersects_int_box(bx) || b2.intersects_int_box(bx)) {
                        minimum_clearance = minimum_clearance.min(*base);
                    }
                }
            }
            let shortfall = minimum_clearance - actual_clearance;
            let comm = &board.communication;
            let board_unit_to_um = Unit::scale(1.0, comm.unit, Unit::Um) / comm.resolution.max(1) as f64;
            let mut tolerance_um = board.rules.clearance_tolerance_um;
            if !tolerance_um.is_finite() || tolerance_um < 0.0 {
                tolerance_um = 0.0;
            }
            let shortfall_um = shortfall * board_unit_to_um;
            if shortfall_um > tolerance_um {
                if *smallest_clearance < 0.0 || actual_clearance < *smallest_clearance {
                    *smallest_clearance = actual_clearance;
                }
                result.push(ClearanceViolation {
                    first_item: key,
                    second_item: other_key,
                    shape: intersection,
                    layer,
                    expected_clearance: minimum_clearance,
                    actual_clearance,
                });
            }
        }
    }
    result
}

/// Java `Item.calculateClearanceBetweenTwoShapes`: bisection (16 steps) for the largest
/// enlargement at which the shapes do not overlap.
fn clearance_between_two_shapes(raw1: &TileShape, raw2: &TileShape, minimum_clearance: f64, cl_comp1: i32, cl_comp2: i32) -> f64 {
    if raw1.intersection(raw2).dimension() == 2 {
        return 0.0;
    }
    let mut low = 0.0;
    let mut high = minimum_clearance;
    let sum_comp = (cl_comp1 + cl_comp2) as f64;
    let factor1 = if sum_comp > 0.0 { cl_comp1 as f64 / sum_comp } else { 0.5 };
    let factor2 = if sum_comp > 0.0 { cl_comp2 as f64 / sum_comp } else { 0.5 };
    for _ in 0..16 {
        let mid = (low + high) * 0.5;
        let s1 = raw1.enlarge(mid * factor1);
        let s2 = raw2.enlarge(mid * factor2);
        if s1.intersection(&s2).dimension() == 2 {
            high = mid;
        } else {
            low = mid;
        }
    }
    low
}

/// Java `Item.outlineContainsTileShape`: every corner of the shape is inside the outline.
fn outline_contains_tile_shape(outline: &BoardOutline, tile_shape: &TileShape) -> bool {
    (0..tile_shape.border_line_count()).all(|ci| outline.contains(&tile_shape.corner(ci)))
}

/// Java `ClearanceViolation.aggregateSortedBySeverity(items)`: the (not deduplicated) violations
/// of the items, stably sorted by descending shortfall. Updates `smallest_clearance` of the items
/// like Java.
pub fn aggregate_sorted_by_severity(board: &mut BasicBoard, items: &[ItemKey]) -> Vec<ClearanceViolation> {
    let mut violations = Vec::new();
    for &k in items {
        violations.extend(clearance_violations_updating(board, k));
    }
    // -Double.compare(a, b) == Double.compare(b, a)
    violations.sort_by(|a, b| {
        let sa = a.expected_clearance - a.actual_clearance;
        let sb = b.expected_clearance - b.actual_clearance;
        java_double_compare(sb, sa)
    });
    violations
}

/// Java `ClearanceViolation.smallestClearance(items)`.
pub fn smallest_clearance(board: &BasicBoard, items: &[ItemKey]) -> f64 {
    let mut smallest = f64::MAX;
    for &k in items {
        let s = board.item(k).smallest_clearance;
        if s >= 0.0 && s < smallest {
            smallest = s;
        }
    }
    smallest
}

/// Java `Double.compare` (total order: -0.0 < 0.0, NaN largest).
pub(crate) fn java_double_compare(a: f64, b: f64) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    if a < b {
        return Ordering::Less;
    }
    if a > b {
        return Ordering::Greater;
    }
    let ab = fr_jcompat::double_to_long_bits(a);
    let bb = fr_jcompat::double_to_long_bits(b);
    ab.cmp(&bb)
}
