//! Port of `rules/BoardRules.java`: rules and constraints for inserting items into a board.
//!
//! Board-dependent parts (TODO, implement on the Board):
//! * `changeClearanceClassIndex(from, to, Collection<Item> boardItems)`: first every board item
//!   with `clearanceClassIndex() == from` gets `setClearanceClassIndex(to)` (item list order),
//!   then the rules part, [`BoardRules::change_clearance_class_index`].
//! * `removeClearanceClass(index, boardItems)`: return false if any board item uses `index`;
//!   otherwise call [`BoardRules::remove_clearance_class`] and, if it returned true, decrement
//!   the clearance class of every item with a class `> index`. (Java decrements the items
//!   before the rules; the data is independent, so the order does not matter.)
//!
//! Methods that need padstack geometry take the board library's [`Padstacks`].

use super::clearance_matrix::ClearanceMatrix;
use super::default_item_clearance_classes::ItemClass;
use super::net::{Net, Nets};
use super::net_class::{NetClass, NetClassId, NetClasses};
use super::via::{ViaInfos, ViaRule, ViaRuleId, ViaRules};
use crate::ids::{AngleRestriction, ClearanceClassNo, LayerNo, NetNo};
use crate::library::Padstacks;
use crate::structure::LayerStructure;

/// Board rules (Java `BoardRules`).
#[derive(Clone, Debug)]
pub struct BoardRules {
    /// Spacing restrictions between item clearance classes.
    pub clearance_matrix: ClearanceMatrix,
    /// The electrical nets.
    pub nets: Nets,
    pub via_infos: ViaInfos,
    pub via_rules: ViaRules,
    pub net_classes: NetClasses,
    layer_structure: LayerStructure,
    trace_angle_restriction: AngleRestriction,
    /// If true, the router ignores conduction areas.
    ignore_conduction: bool,
    /// Smallest of all default trace half widths set through the rules.
    min_trace_half_width: i32,
    /// Biggest of all default trace half widths set through the rules.
    max_trace_half_width: i32,
    /// Minimum distance of the pad border to the first turn of a trace at pins with restricted
    /// exit directions; `<= 0` means no restriction.
    pin_edge_to_turn_dist: f64,
    use_slow_autoroute_algorithm: bool,
    hole_clearance: i32,
    /// Minimum drill-to-drill gap of two vias of the same net, in board units (fastroute extension;
    /// `router.same_net_via_hole_to_hole_um`). None: such pairs are judged by the copper clearance.
    same_net_via_hole_to_hole: Option<f64>,
    /// (net class name, box in board units, base clearance in board units): fastroute extension
    /// (`router.clearance_exempt_areas`), used by the clearance check only.
    pub clearance_exempt_areas: Vec<(String, fr_geom::IntBox, f64)>,
    /// fastroute fixes to the clearance report, false on a bare board (Java behaviour); the CLI turns
    /// them on with its enhancements (off only with --parity / --no-enhancements).
    pub exact_clearance_check: bool,
    pub skip_same_component_pin_pairs: bool,
    /// DRC clearance tolerance in micrometers (transient in Java, default 1.0).
    pub clearance_tolerance_um: f64,
}

impl BoardRules {
    /// Java `defaultClearanceClass()`.
    pub const DEFAULT_CLEARANCE_CLASS: ClearanceClassNo = 1;
    /// Java `clearanceClassNone()`: the class of items without clearance.
    pub const CLEARANCE_CLASS_NONE: ClearanceClassNo = 0;

    pub fn new(layer_structure: LayerStructure, clearance_matrix: ClearanceMatrix) -> Self {
        BoardRules {
            clearance_matrix,
            nets: Nets::new(),
            via_infos: ViaInfos::new(),
            via_rules: ViaRules::new(),
            net_classes: NetClasses::new(),
            layer_structure,
            trace_angle_restriction: AngleRestriction::FortyfiveDegree,
            ignore_conduction: true,
            min_trace_half_width: 100_000,
            max_trace_half_width: 100,
            pin_edge_to_turn_dist: 0.0,
            use_slow_autoroute_algorithm: false,
            hole_clearance: 0,
            same_net_via_hole_to_hole: None,
            clearance_exempt_areas: Vec::new(),
            exact_clearance_check: false,
            skip_same_component_pin_pairs: false,
            clearance_tolerance_um: 1.0,
        }
    }

    /// Java `defaultClearanceClass()`.
    pub fn default_clearance_class() -> ClearanceClassNo {
        Self::DEFAULT_CLEARANCE_CLASS
    }

    /// Java `clearanceClassNone()`.
    pub fn clearance_class_none() -> ClearanceClassNo {
        Self::CLEARANCE_CLASS_NONE
    }

    pub fn layer_structure(&self) -> &LayerStructure {
        &self.layer_structure
    }

    /// The net with the given number (panics if missing, Java NPE at the call sites).
    fn net(&self, net_number: NetNo) -> &Net {
        self.nets
            .get(net_number)
            .unwrap_or_else(|| panic!("BoardRules: net {net_number} not found"))
    }

    /// The net class of the given net (panics if the net does not exist).
    pub fn net_class_of(&self, net_number: NetNo) -> &NetClass {
        &self.net_classes[self.net(net_number).get_net_class()]
    }

    /// Routing trace half width of the net on `layer` (panics if the net does not exist).
    pub fn get_trace_half_width(&self, net_number: NetNo, layer: LayerNo) -> i32 {
        self.net_class_of(net_number).get_trace_half_width(layer)
    }

    /// True if the routing trace widths of the net differ between layers.
    pub fn trace_widths_are_layer_dependent(&self, net_number: NetNo) -> bool {
        let compare_width = self.get_trace_half_width(net_number, 0);
        (1..self.layer_structure.layer_count())
            .any(|i| self.get_trace_half_width(net_number, i) != compare_width)
    }

    pub fn get_min_trace_half_width(&self) -> i32 {
        self.min_trace_half_width
    }

    pub fn get_max_trace_half_width(&self) -> i32 {
        self.max_trace_half_width
    }

    /// The clearance around drilled holes.
    pub fn get_hole_clearance(&self) -> i32 {
        self.hole_clearance
    }

    pub fn set_hole_clearance(&mut self, value: i32) {
        self.hole_clearance = value.max(0);
    }

    pub fn get_same_net_via_hole_to_hole(&self) -> Option<f64> {
        self.same_net_via_hole_to_hole
    }

    pub fn set_same_net_via_hole_to_hole(&mut self, value: Option<f64>) {
        self.same_net_via_hole_to_hole = value.map(|v| v.max(0.0));
    }

    /// Changes the default trace half width on `layer` and updates the min/max.
    pub fn set_default_trace_half_width(&mut self, layer: LayerNo, value: i32) {
        let d = self.get_default_net_class();
        self.net_classes[d].set_trace_half_width_on_layer(layer, value);
        self.min_trace_half_width = self.min_trace_half_width.min(value);
        self.max_trace_half_width = self.max_trace_half_width.max(value);
    }

    /// The default trace half width on `layer`. Requires the default net class to exist (Java
    /// would create it lazily; see [`Self::get_default_net_class`]).
    pub fn get_default_trace_half_width(&self, layer: LayerNo) -> i32 {
        self.net_classes[self.default_net_class()].get_trace_half_width(layer)
    }

    /// Changes the default trace half width on all layers (ignored with a warning if `<= 0`).
    pub fn set_default_trace_half_widths(&mut self, value: i32) {
        if value <= 0 {
            log::warn!("BoardRules.set_trace_half_widths: value out of range");
            return;
        }
        let d = self.get_default_net_class();
        self.net_classes[d].set_trace_half_width(value);
        self.min_trace_half_width = self.min_trace_half_width.min(value);
        self.max_trace_half_width = self.max_trace_half_width.max(value);
    }

    /// The net class used for nets without a special class (list index 0), creating it if no
    /// class exists yet (Java `getDefaultNetClass`).
    pub fn get_default_net_class(&mut self) -> NetClassId {
        if self.net_classes.count() <= 0 {
            self.create_default_net_class();
        }
        self.net_classes.get(0)
    }

    /// Non-creating variant of [`Self::get_default_net_class`]; panics if no net class exists.
    pub fn default_net_class(&self) -> NetClassId {
        assert!(
            self.net_classes.count() > 0,
            "BoardRules: net classes not initialized"
        );
        self.net_classes.get(0)
    }

    /// Appends a class `class<n>` initialized from the default class (Java `getNewNetClass()`).
    pub fn get_new_net_class(&mut self) -> NetClassId {
        let result = self.net_classes.append_unnamed(&self.layer_structure);
        self.init_new_net_class(result);
        result
    }

    /// Appends a class with the given name initialized from the default class (Java
    /// `getNewNetClass(String)`; does not check for an existing name).
    pub fn get_new_net_class_named(&mut self, name: &str) -> NetClassId {
        let result = self.net_classes.append(name, &self.layer_structure, false);
        self.init_new_net_class(result);
        result
    }

    fn init_new_net_class(&mut self, result: NetClassId) {
        let d = self.get_default_net_class();
        let cl = self.net_classes[d].get_trace_clearance_class();
        self.net_classes[result].set_trace_clearance_class(cl);
        let via_rule = self.get_default_via_rule();
        self.net_classes[result].set_via_rule(via_rule);
        let d = self.get_default_net_class();
        let w = self.net_classes[d].get_trace_half_width(0);
        self.net_classes[result].set_trace_half_width(w);
    }

    /// Creates a via rule `name` for `net_class` from all via infos with the class's default via
    /// clearance class; of several vias with the same layer range only the one with the
    /// smallest pad (`max_width` on its first layer) is kept. Appends the rule and assigns it to
    /// the class. Does nothing if there are no via infos.
    pub fn create_default_via_rule(
        &mut self,
        net_class: NetClassId,
        name: &str,
        padstacks: &Padstacks,
    ) {
        if self.via_infos.count() == 0 {
            return;
        }
        let mut default_rule = ViaRule::new(name);
        let default_via_cl_class = self.net_classes[net_class]
            .default_item_clearance_classes
            .get(ItemClass::Via);
        for i in 0..self.via_infos.count() {
            let current_via_info = self.via_infos.get(i);
            if self.via_infos[current_via_info].get_clearance_class_index() != default_via_cl_class
            {
                continue;
            }
            let current_padstack = padstacks
                .get(self.via_infos[current_via_info].get_padstack())
                .expect("BoardRules.createDefaultViaRule: padstack not found");
            let current_from_layer = current_padstack.from_layer();
            let current_to_layer = current_padstack.to_layer();
            match default_rule.get_layer_range(
                current_from_layer,
                current_to_layer,
                &self.via_infos,
                padstacks,
            ) {
                Some(existing_via) => {
                    let new_shape = current_padstack
                        .get_shape(current_from_layer)
                        .expect("via shape is null");
                    let existing_padstack = padstacks
                        .get(self.via_infos[existing_via].get_padstack())
                        .expect("padstack not found");
                    let existing_shape = existing_padstack
                        .get_shape(current_from_layer)
                        .expect("via shape is null");
                    if new_shape.max_width() < existing_shape.max_width() {
                        // The via with the smallest pad shape is preferred.
                        default_rule.remove_via(existing_via);
                        default_rule.append_via(current_via_info);
                    }
                }
                None => default_rule.append_via(current_via_info),
            }
        }
        let rule = self.via_rules.add(default_rule);
        self.net_classes[net_class].set_via_rule(Some(rule));
    }

    /// Creates the default net class "default" (half width 1500, trace clearance class 1).
    pub fn create_default_net_class(&mut self) {
        let default_net_class = self
            .net_classes
            .append("default", &self.layer_structure, false);
        let default_trace_half_width = 1500;
        self.net_classes[default_net_class].set_trace_half_width(default_trace_half_width);
        self.net_classes[default_net_class].set_trace_clearance_class(1);
    }

    /// Appends a class `class<n>` with via rule, trace width and clearance class of list
    /// index 0 (Java `appendNetClass()`).
    pub fn append_net_class_unnamed(&mut self) -> NetClassId {
        let new_class = self.net_classes.append_unnamed(&self.layer_structure);
        let default_class = self.net_classes.get(0);
        let (via_rule, w, cl) = {
            let d = &self.net_classes[default_class];
            (
                d.get_via_rule(),
                d.get_trace_half_width(0),
                d.get_trace_clearance_class(),
            )
        };
        let n = &mut self.net_classes[new_class];
        n.set_via_rule(via_rule);
        n.set_trace_half_width(w);
        n.set_trace_clearance_class(cl);
        new_class
    }

    /// Returns the class named `name` (case-sensitive) if it exists; otherwise appends it,
    /// initialized from list index 0 including its default item clearance classes (Java
    /// `appendNetClass(String)`).
    pub fn append_net_class(&mut self, name: &str) -> NetClassId {
        if let Some(found) = self.net_classes.get_by_name(name) {
            return found;
        }
        let new_class = self.net_classes.append(name, &self.layer_structure, false);
        let default_class = self.net_classes.get(0);
        let d = self.net_classes[default_class].clone();
        let n = &mut self.net_classes[new_class];
        n.default_item_clearance_classes = d.default_item_clearance_classes.clone();
        n.set_via_rule(d.get_via_rule());
        n.set_trace_half_width(d.get_trace_half_width(0));
        n.set_trace_clearance_class(d.get_trace_clearance_class());
        new_class
    }

    /// Adds a net with the default net class (Java `Nets.add`, whose `Net` constructor calls
    /// `board.rules.getDefaultNetClass()`); returns the net number.
    pub fn add_net(&mut self, name: &str, subnet_number: i32, contains_plane: bool) -> NetNo {
        let default_class = self.get_default_net_class();
        self.nets
            .add(name, subnet_number, contains_plane, default_class)
    }

    /// The default via rule for routing (first rule), if any.
    pub fn get_default_via_rule(&self) -> Option<ViaRuleId> {
        self.via_rules.get_first()
    }

    /// The via rule with the given name (case-sensitive), if any.
    pub fn get_via_rule(&self, name: &str) -> Option<ViaRuleId> {
        self.via_rules
            .iter()
            .find(|&id| self.via_rules[id].name == name)
    }

    /// Rules part of Java `changeClearanceClassIndex`: replaces clearance class `from_index` by
    /// `to_index` in net classes (trace class and default item classes) and via infos. The
    /// board items must be updated by the caller first (see module docs).
    pub fn change_clearance_class_index(
        &mut self,
        from_index: ClearanceClassNo,
        to_index: ClearanceClassNo,
    ) {
        for i in 0..self.net_classes.count() {
            let id = self.net_classes.get(i);
            let c = &mut self.net_classes[id];
            if c.get_trace_clearance_class() == from_index {
                c.set_trace_clearance_class(to_index);
            }
            for item_class in ItemClass::VALUES {
                if c.default_item_clearance_classes.get(item_class) == from_index {
                    c.default_item_clearance_classes.set(item_class, to_index);
                }
            }
        }
        for i in 0..self.via_infos.count() {
            let id = self.via_infos.get(i);
            if self.via_infos[id].get_clearance_class_index() == from_index {
                self.via_infos[id].set_clearance_class_index(to_index);
            }
        }
    }

    /// Rules part of Java `removeClearanceClass`: false if a net class or via info still uses
    /// `index`; otherwise decrements all larger class numbers in net classes and via infos,
    /// removes the class from the clearance matrix and returns true. The caller must check and
    /// renumber the board items (see module docs).
    pub fn remove_clearance_class(&mut self, index: ClearanceClassNo) -> bool {
        for id in self.net_classes.iter() {
            let c = &self.net_classes[id];
            if c.get_trace_clearance_class() == index {
                return false;
            }
            if ItemClass::VALUES
                .iter()
                .any(|&ic| c.default_item_clearance_classes.get(ic) == index)
            {
                return false;
            }
        }
        if self
            .via_infos
            .iter()
            .any(|id| self.via_infos[id].get_clearance_class_index() == index)
        {
            return false;
        }
        for i in 0..self.net_classes.count() {
            let id = self.net_classes.get(i);
            let c = &mut self.net_classes[id];
            if c.get_trace_clearance_class() > index {
                c.set_trace_clearance_class(c.get_trace_clearance_class() - 1);
            }
            for item_class in ItemClass::VALUES {
                let current_class_no = c.default_item_clearance_classes.get(item_class);
                if current_class_no > index {
                    c.default_item_clearance_classes
                        .set(item_class, current_class_no - 1);
                }
            }
        }
        for i in 0..self.via_infos.count() {
            let id = self.via_infos.get(i);
            let v = &mut self.via_infos[id];
            if v.get_clearance_class_index() > index {
                v.set_clearance_class_index(v.get_clearance_class_index() - 1);
            }
        }
        self.clearance_matrix.remove_class(index);
        true
    }

    /// Minimum distance between the pin border and the next trace corner for pins with exit
    /// restrictions; `<= 0` means no restrictions.
    pub fn get_pin_edge_to_turn_dist(&self) -> f64 {
        self.pin_edge_to_turn_dist
    }

    pub fn set_pin_edge_to_turn_dist(&mut self, value: f64) {
        self.pin_edge_to_turn_dist = value;
    }

    /// Whether the router ignores conduction areas (default true).
    pub fn get_ignore_conduction(&self) -> bool {
        self.ignore_conduction
    }

    pub fn set_ignore_conduction(&mut self, value: bool) {
        self.ignore_conduction = value;
    }

    /// The trace angle restriction (default 45 degree).
    pub fn get_trace_angle_restriction(&self) -> AngleRestriction {
        self.trace_angle_restriction
    }

    pub fn set_trace_angle_restriction(&mut self, angle_restriction: AngleRestriction) {
        self.trace_angle_restriction = angle_restriction;
    }

    /// If true, the autorouter always uses Simplex shapes; else IntBox (90 degree) or
    /// IntOctagon (45 degree).
    pub fn get_use_slow_autoroute_algorithm(&self) -> bool {
        self.use_slow_autoroute_algorithm
    }

    pub fn set_use_slow_autoroute_algorithm(&mut self, value: bool) {
        self.use_slow_autoroute_algorithm = value;
    }

    /// The maximum diameter of the default via on its first and last layer (0 if none).
    pub fn get_default_via_diameter(&self, padstacks: &Padstacks) -> f64 {
        let Some(default_via_rule) = self.get_default_via_rule() else {
            return 0.0;
        };
        let rule = &self.via_rules[default_via_rule];
        if rule.via_count() <= 0 {
            return 0.0;
        }
        let via_padstack = padstacks
            .get(self.via_infos[rule.get_via(0)].get_padstack())
            .expect("BoardRules.getDefaultViaDiameter: padstack not found");
        let result = via_padstack
            .get_shape(via_padstack.from_layer())
            .expect("via shape is null")
            .max_width();
        let other = via_padstack
            .get_shape(via_padstack.to_layer())
            .expect("via shape is null")
            .max_width();
        // Java Math.max(double, double); widths are never NaN.
        if other > result {
            other
        } else {
            result
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::{DefaultItemClearanceClasses, ViaInfo};
    use crate::structure::Layer;
    use fr_geom::{Circle, ConvexShape, IntPoint};

    fn ls(n: usize) -> LayerStructure {
        LayerStructure::new((0..n).map(|i| Layer::new(format!("L{i}"), true)).collect())
    }

    fn rules(n: usize) -> BoardRules {
        let ls = ls(n);
        let m = ClearanceMatrix::get_default_instance(&ls, 0);
        BoardRules::new(ls, m)
    }

    #[test]
    fn defaults() {
        let mut r = rules(2);
        assert_eq!(BoardRules::default_clearance_class(), 1);
        assert_eq!(BoardRules::clearance_class_none(), 0);
        assert_eq!(
            r.get_trace_angle_restriction(),
            AngleRestriction::FortyfiveDegree
        );
        assert!(r.get_ignore_conduction());
        assert_eq!(
            (r.get_min_trace_half_width(), r.get_max_trace_half_width()),
            (100_000, 100)
        );
        assert_eq!(r.net_classes.count(), 0);
        let d = r.get_default_net_class();
        assert_eq!(r.net_classes.count(), 1);
        assert_eq!(r.net_classes[d].get_name(), "default");
        assert_eq!(r.net_classes[d].get_trace_half_width(1), 1500);
        assert_eq!(r.net_classes[d].get_trace_clearance_class(), 1);
        // creating the default class does not touch min/max
        assert_eq!(r.get_min_trace_half_width(), 100_000);
        r.set_default_trace_half_widths(0);
        assert_eq!(r.get_default_trace_half_width(0), 1500);
        r.set_default_trace_half_widths(250);
        r.set_default_trace_half_width(1, 400);
        assert_eq!(
            (r.get_min_trace_half_width(), r.get_max_trace_half_width()),
            (250, 400)
        );
        r.set_hole_clearance(-5);
        assert_eq!(r.get_hole_clearance(), 0);
    }

    #[test]
    fn nets_get_default_class() {
        let mut r = rules(3);
        let n = r.add_net("GND", 1, false);
        assert_eq!(n, 1);
        assert_eq!(r.net_classes.count(), 1);
        assert_eq!(r.get_trace_half_width(1, 2), 1500);
        assert!(!r.trace_widths_are_layer_dependent(1));
        r.set_default_trace_half_width(2, 10);
        assert!(r.trace_widths_are_layer_dependent(1));
        let c = r.append_net_class("power");
        r.nets.get_mut(1).unwrap().set_class(c);
        assert_eq!(r.get_trace_half_width(1, 2), 1500);
        assert_eq!(r.append_net_class("power"), c);
    }

    #[test]
    fn append_net_class_without_default() {
        // Java appendNetClass(name) on empty classes makes the new class index 0.
        let mut r = rules(2);
        let c = r.append_net_class("first");
        assert_eq!(r.net_classes.get(0), c);
        assert_eq!(r.get_default_net_class(), c);
        assert_eq!(r.net_classes[c].get_trace_half_width(0), 0);
        // getNewNetClass(name) on empty classes: the new class itself becomes the default
        let mut r = rules(2);
        let c = r.get_new_net_class_named("x");
        assert_eq!(r.net_classes.count(), 1);
        assert_eq!(r.default_net_class(), c);
    }

    #[test]
    fn new_net_classes_copy_default() {
        let mut r = rules(2);
        let d = r.get_default_net_class();
        r.net_classes[d]
            .default_item_clearance_classes
            .set(ItemClass::Via, 3);
        let a = r.get_new_net_class();
        assert_eq!(r.net_classes[a].get_name(), "class1");
        assert_eq!(r.net_classes[a].get_trace_half_width(1), 1500);
        assert_eq!(r.net_classes[a].get_trace_clearance_class(), 1);
        // getNewNetClass/appendNetClass() do not copy the default item clearance classes
        assert_eq!(
            r.net_classes[a].default_item_clearance_classes,
            DefaultItemClearanceClasses::new()
        );
        let b = r.append_net_class_unnamed();
        assert_eq!(r.net_classes[b].get_name(), "class2");
        let c = r.append_net_class("named");
        assert_eq!(
            r.net_classes[c]
                .default_item_clearance_classes
                .get(ItemClass::Via),
            3
        );
    }

    fn via_padstacks(n: usize) -> (Padstacks, [i32; 4]) {
        let mut ps = Padstacks::new(&ls(n));
        let circ = |r| ConvexShape::Circle(Circle::new(IntPoint::new(0, 0), r));
        let big = ps.add_shape_range(&circ(400), 0, 3);
        let small = ps.add_shape_range(&circ(300), 0, 3);
        let blind = ps.add_shape_range(&circ(200), 0, 1);
        let smaller = ps.add_shape_range(&circ(250), 0, 3);
        (ps, [big, small, blind, smaller])
    }

    #[test]
    fn default_via_rule() {
        let (ps, [big, small, blind, smaller]) = via_padstacks(4);
        let mut r = rules(4);
        let d = r.get_default_net_class();
        r.create_default_via_rule(d, "default", &ps);
        assert!(r.get_default_via_rule().is_none());
        assert_eq!(r.get_default_via_diameter(&ps), 0.0);

        let v_big = r.via_infos.add(ViaInfo::new("big", big, 1, false)).unwrap();
        let v_small = r
            .via_infos
            .add(ViaInfo::new("small", small, 1, false))
            .unwrap();
        let v_blind = r
            .via_infos
            .add(ViaInfo::new("blind", blind, 1, false))
            .unwrap();
        let _v_other = r
            .via_infos
            .add(ViaInfo::new("other_class", smaller, 2, false))
            .unwrap();
        r.create_default_via_rule(d, "default", &ps);
        let rule = r.get_default_via_rule().unwrap();
        assert_eq!(r.net_classes[d].get_via_rule(), Some(rule));
        // big replaced by small (removed, small appended at the end), other class ignored
        assert_eq!(r.via_rules[rule].vias(), &[v_small, v_blind]);
        assert!(!r.via_rules[rule].contains(v_big));
        assert_eq!(r.get_via_rule("default"), Some(rule));
        assert_eq!(r.get_via_rule("Default"), None);
        assert_eq!(r.get_default_via_diameter(&ps), 600.0);
        // new classes get the default via rule
        let a = r.get_new_net_class();
        assert_eq!(r.net_classes[a].get_via_rule(), Some(rule));
    }

    #[test]
    fn clearance_class_changes() {
        let mut r = rules(2);
        r.clearance_matrix.append_class("a");
        r.clearance_matrix.append_class("b");
        let d = r.get_default_net_class();
        let c = r.append_net_class("c");
        r.net_classes[c].set_trace_clearance_class(3);
        r.net_classes[c]
            .default_item_clearance_classes
            .set(ItemClass::Smd, 3);
        let v = r.via_infos.add(ViaInfo::new("v", 1, 3, false)).unwrap();
        assert!(!r.remove_clearance_class(3));
        assert!(!r.remove_clearance_class(1));
        assert!(r.remove_clearance_class(2));
        assert_eq!(r.clearance_matrix.get_class_count(), 3);
        assert_eq!(r.clearance_matrix.get_name(2), Some("b"));
        assert_eq!(r.net_classes[c].get_trace_clearance_class(), 2);
        assert_eq!(
            r.net_classes[c]
                .default_item_clearance_classes
                .get(ItemClass::Smd),
            2
        );
        assert_eq!(
            r.net_classes[c]
                .default_item_clearance_classes
                .get(ItemClass::Pin),
            1
        );
        assert_eq!(r.via_infos[v].get_clearance_class_index(), 2);
        r.change_clearance_class_index(1, 2);
        assert_eq!(r.net_classes[d].get_trace_clearance_class(), 2);
        assert_eq!(
            r.net_classes[d]
                .default_item_clearance_classes
                .get(ItemClass::Trace),
            2
        );
        assert_eq!(
            r.net_classes[d]
                .default_item_clearance_classes
                .get(ItemClass::None),
            0
        );
        assert!(r.remove_clearance_class(1));
        assert_eq!(r.net_classes[d].get_trace_clearance_class(), 1);
    }
}
