//! The connectivity parts of `Item`, `DrillItem`, `Trace`, `ConductionArea` and
//! `BoardConnectivityQueries`: normal contacts, contact points, connected sets, connection
//! items, tails and cycles.
//!
//! The Java recursions (`getConnectedSetRecu`, `isCycleRecu`) are iterative here with an
//! explicit stack that visits the contacts in the same order (depth first, contacts in
//! `TreeSet` order), so deep nets cannot overflow the stack.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::OnceLock;

use fr_geom::{ConvexShape, FloatPoint, Point, TileShape};

use crate::ids::{FixedState, ItemId, LayerNo, NetNo};

use super::basic_board::BasicBoard;
use super::item::{Item, ItemKey, ItemKind, StopConnectionOption};
use super::item_list::ItemSet;
use super::search_tree::TreeObject;

thread_local! {
    static CONTACTS: RefCell<ContactsCache> = RefCell::new(ContactsCache::default());
}

/// Per thread cache of normal contacts for the last few board contents (keyed by the epochs of
/// the item repository and the default tree).
#[derive(Default)]
struct ContactsCache {
    slots: Vec<((u64, u64), ContactsMap)>,
}

type ContactsMap = HashMap<ItemKey, Rc<[ItemKey]>>;

const CONTACTS_CACHE_SLOTS: usize = 4;

impl ContactsCache {
    fn get(&mut self, epochs: (u64, u64), key: ItemKey) -> Option<Rc<[ItemKey]>> {
        let slot = self.slots.iter().find(|(e, _)| *e == epochs)?;
        slot.1.get(&key).cloned()
    }

    fn put(&mut self, epochs: (u64, u64), key: ItemKey, value: Rc<[ItemKey]>) {
        let pos = match self.slots.iter().position(|(e, _)| *e == epochs) {
            Some(p) => p,
            None => {
                if self.slots.len() >= CONTACTS_CACHE_SLOTS {
                    self.slots.remove(0);
                }
                self.slots.push((epochs, HashMap::new()));
                self.slots.len() - 1
            }
        };
        self.slots[pos].1.insert(key, value);
    }
}

/// `FASTROUTE_VERIFY_CACHES=1`: recompute every cached result and assert that it is unchanged.
pub(crate) fn verify_caches() -> bool {
    static V: OnceLock<bool> = OnceLock::new();
    *V.get_or_init(|| std::env::var_os("FASTROUTE_VERIFY_CACHES").is_some_and(|v| v != "0"))
}

/// Java `Item.PROTECT_FANOUT_LENGTH`.
const PROTECT_FANOUT_LENGTH: f64 = 400.0;

fn point_shape(point: &Point) -> ConvexShape {
    ConvexShape::Tile(TileShape::IntBox(TileShape::get_instance_point(point)))
}

impl BasicBoard {
    /// Java `Trace.getNormalContacts(point, ignoreNet)`: the items with a connection point at
    /// `point` on the trace layer.
    pub fn trace_normal_contacts_at(&self, key: ItemKey, point: &Point, ignore_net: bool) -> ItemSet {
        let this = self.item(key);
        let mut result = ItemSet::new();
        if !(*point == this.first_corner() || *point == this.last_corner()) {
            return result;
        }
        let layer = this.trace().layer;
        for o in self.overlapping_objects(&point_shape(point), layer) {
            let TreeObject::Item { key: other_key, id } = o else { continue };
            if other_key == key {
                continue;
            }
            let other = self.item(other_key);
            if !(other.shares_layer(this, self) && (ignore_net || other.shares_net(this))) {
                continue;
            }
            let contact = match &other.kind {
                ItemKind::Trace(_) => *point == other.first_corner() || *point == other.last_corner(),
                ItemKind::Pin(_) | ItemKind::Via(_) => *point == other.center(self),
                ItemKind::ConductionArea(c) => c.area.get_area(self).contains(point),
                _ => false,
            };
            if contact {
                result.insert(ItemId(id), other_key);
            }
        }
        result
    }

    /// Java `Trace.getStartContacts()`.
    pub fn trace_start_contacts(&self, key: ItemKey) -> ItemSet {
        self.trace_normal_contacts_at(key, &self.item(key).first_corner(), false)
    }

    /// Java `Trace.getEndContacts()`.
    pub fn trace_end_contacts(&self, key: ItemKey) -> ItemSet {
        self.trace_normal_contacts_at(key, &self.item(key).last_corner(), false)
    }

    /// Java `getNormalContacts()` (dispatching on the item class).
    pub fn normal_contacts(&self, key: ItemKey) -> ItemSet {
        let mut result = ItemSet::new();
        for &k in self.normal_contacts_keys(key).iter() {
            result.insert(self.item(k).id(), k);
        }
        result
    }

    /// The keys of [`Self::normal_contacts`] in set order, memoized per board content (see
    /// [`super::epoch`]): the contacts of an item depend only on the items and the item entries
    /// of the default search tree.
    pub fn normal_contacts_keys(&self, key: ItemKey) -> Rc<[ItemKey]> {
        let epochs = (self.items.epoch(), self.default_tree().item_epoch());
        if let Some(hit) = CONTACTS.with(|c| c.borrow_mut().get(epochs, key)) {
            if verify_caches() {
                let fresh: Vec<ItemKey> = self.normal_contacts_uncached(key).iter().collect();
                assert_eq!(&fresh[..], &hit[..], "normal contacts cache out of date");
            }
            return hit;
        }
        let keys: Rc<[ItemKey]> = self.normal_contacts_uncached(key).iter().collect();
        CONTACTS.with(|c| c.borrow_mut().put(epochs, key, keys.clone()));
        keys
    }

    /// Java `getNormalContacts()` computed without the cache.
    pub fn normal_contacts_uncached(&self, key: ItemKey) -> ItemSet {
        let this = self.item(key);
        match &this.kind {
            ItemKind::Trace(_) => {
                let mut result = ItemSet::new();
                result.extend_from(&self.trace_normal_contacts_at(key, &this.first_corner(), false));
                result.extend_from(&self.trace_normal_contacts_at(key, &this.last_corner(), false));
                result
            }
            ItemKind::Pin(_) | ItemKind::Via(_) => self.drill_normal_contacts(key),
            ItemKind::ConductionArea(_) => self.conduction_area_normal_contacts(key),
            _ => ItemSet::new(),
        }
    }

    fn drill_normal_contacts(&self, key: ItemKey) -> ItemSet {
        let this = self.item(key);
        let drill_center = this.center(self);
        let mut result = ItemSet::new();
        for o in self.overlapping_objects(&point_shape(&drill_center), -1) {
            let TreeObject::Item { key: other_key, id } = o else { continue };
            if other_key == key {
                continue;
            }
            let other = self.item(other_key);
            if !(other.shares_net(this) && other.shares_layer(this, self)) {
                continue;
            }
            let contact = match &other.kind {
                // exact matching of trace endpoints to the drill center
                ItemKind::Trace(_) => drill_center == other.first_corner() || drill_center == other.last_corner(),
                ItemKind::Pin(_) | ItemKind::Via(_) => drill_center == other.center(self),
                ItemKind::ConductionArea(c) => c.area.get_area(self).contains(&drill_center),
                _ => false,
            };
            if contact {
                result.insert(ItemId(id), other_key);
            }
        }
        if self.overlap_contacts {
            // fastroute: overlapping copper of pins and vias of the net (pads that touch, a via
            // in a pad off its center). The maze search already treats them as connected; with
            // the center rule they stayed "unrouted" and were retried in every pass. (Trace ends
            // keep the exact rule: the pull-tight and tail logic rely on ends at the center.)
            let first = this.first_layer(self);
            for i in 0..self.tile_shape_count(key) {
                let layer = first + i;
                let Some(shape) = self.tile_shape(key, i) else { continue };
                for o in self.overlapping_objects(&ConvexShape::Tile(shape), layer) {
                    let TreeObject::Item { key: other_key, id } = o else { continue };
                    if other_key == key || result.contains(ItemId(id)) {
                        continue;
                    }
                    let other = self.item(other_key);
                    if matches!(other.kind, ItemKind::Pin(_) | ItemKind::Via(_)) && other.shares_net(this) {
                        result.insert(ItemId(id), other_key);
                    }
                }
            }
        }
        result
    }

    fn conduction_area_normal_contacts(&self, key: ItemKey) -> ItemSet {
        let this = self.item(key);
        let layer = this.first_layer(self);
        let mut result = ItemSet::new();
        for i in 0..self.tile_shape_count(key) {
            let Some(current_shape) = self.tile_shape(key, i) else { continue };
            for o in self.overlapping_objects(&ConvexShape::Tile(current_shape.clone()), layer) {
                let TreeObject::Item { key: other_key, id } = o else { continue };
                if other_key == key {
                    continue;
                }
                let other = self.item(other_key);
                if !(other.shares_net(this) && other.shares_layer(this, self)) {
                    continue;
                }
                let contact = match &other.kind {
                    ItemKind::Trace(_) => current_shape.contains(&other.first_corner()) || current_shape.contains(&other.last_corner()),
                    ItemKind::Pin(_) | ItemKind::Via(_) => current_shape.contains(&other.center(self)),
                    _ => false,
                };
                if contact {
                    result.insert(ItemId(id), other_key);
                }
            }
        }
        result
    }

    /// Java `getAllContacts()` (`layer == None`) and `getAllContacts(layer)`.
    pub fn all_contacts(&self, key: ItemKey, layer: Option<LayerNo>) -> ItemSet {
        let this = self.item(key);
        let mut result = ItemSet::new();
        if !this.is_connectable_class() {
            return result;
        }
        for i in 0..self.tile_shape_count(key) {
            let shape_layer = self.shape_layer(key, i);
            if let Some(l) = layer {
                if shape_layer != l {
                    continue;
                }
            }
            let Some(shape) = self.tile_shape(key, i) else { continue };
            for o in self.overlapping_objects(&ConvexShape::Tile(shape), shape_layer) {
                let TreeObject::Item { key: other_key, id } = o else { continue };
                let other = self.item(other_key);
                if other_key != key && other.is_connectable_class() && other.shares_net(this) {
                    result.insert(ItemId(id), other_key);
                }
            }
        }
        result
    }

    /// Java `isConnected()`.
    pub fn is_connected(&self, key: ItemKey) -> bool {
        !self.all_contacts(key, None).is_empty()
    }

    /// Java `isConnectedOnLayer(layer)`.
    pub fn is_connected_on_layer(&self, key: ItemKey, layer: LayerNo) -> bool {
        !self.all_contacts(key, Some(layer)).is_empty()
    }

    /// Java `this.normalContactPoint(other)` (double dispatch between traces and drill items).
    pub fn normal_contact_point(&self, a: ItemKey, b: ItemKey) -> Option<Point> {
        let ia = self.item(a);
        let ib = self.item(b);
        match (&ia.kind, &ib.kind) {
            (ItemKind::Pin(_) | ItemKind::Via(_), ItemKind::Trace(_)) => self.drill_trace_contact_point(ia, ib),
            (ItemKind::Trace(_), ItemKind::Pin(_) | ItemKind::Via(_)) => self.drill_trace_contact_point(ib, ia),
            (ItemKind::Pin(_) | ItemKind::Via(_), ItemKind::Pin(_) | ItemKind::Via(_)) => {
                // other.normalContactPoint((DrillItem) this) with this = b
                let cb = ib.center(self);
                if ib.shares_layer(ia, self) && cb == ia.center(self) {
                    Some(cb)
                } else {
                    None
                }
            }
            (ItemKind::Trace(_), ItemKind::Trace(_)) => trace_trace_contact_point(ib, ia),
            _ => None,
        }
    }

    /// Java `DrillItem.normalContactPoint(Trace)` with `this = drill`.
    fn drill_trace_contact_point(&self, drill: &Item, trace: &Item) -> Option<Point> {
        if !drill.shares_layer(trace, self) {
            return None;
        }
        let center = drill.center(self);
        if center == trace.first_corner() || center == trace.last_corner() {
            Some(center)
        } else {
            None
        }
    }

    /// Java `getConnectedSet(netNumber, stopAtPlane)`.
    pub fn connected_set(&self, key: ItemKey, net_number: NetNo, stop_at_plane: bool) -> ItemSet {
        let this = self.item(key);
        let mut result = ItemSet::new();
        if net_number > 0 && !this.contains_net(net_number) {
            return result;
        }
        result.insert(this.id(), key);
        // iterative version of getConnectedSetRecu
        let mut stack: Vec<(Rc<[ItemKey]>, usize)> = vec![(self.normal_contacts_keys(key), 0)];
        if self.is_stitching_via(key) {
            stack.push((self.stitching_via_contacts(key, net_number), 0));
        }
        while let Some((contacts, pos)) = stack.last_mut() {
            if *pos >= contacts.len() {
                stack.pop();
                continue;
            }
            let contact = contacts[*pos];
            *pos += 1;
            let c = self.item(contact);
            if stop_at_plane && c.is_conduction_area() && c.component_no() <= 0 {
                continue;
            }
            if net_number > 0 && !c.contains_net(net_number) {
                continue;
            }
            if result.insert(c.id(), contact) {
                stack.push((self.normal_contacts_keys(contact), 0));
                if self.is_stitching_via(contact) {
                    stack.push((self.stitching_via_contacts(contact, net_number), 0));
                }
            }
        }
        result
    }

    /// fastroute: the other stitching vias of the net of `key`, which the copper zone of the
    /// net joins to it (the zone itself is not on the board, see
    /// [`BasicBoard::mark_stitching_vias`]). Virtual contacts for [`Self::connected_set`].
    fn stitching_via_contacts(&self, key: ItemKey, net_number: NetNo) -> Rc<[ItemKey]> {
        let net = if net_number > 0 { net_number } else { self.item(key).net_numbers().first().copied().unwrap_or(0) };
        if net <= 0 {
            return Rc::from(Vec::new());
        }
        self.items.net_items(net).filter(|&k| k != key && self.is_stitching_via(k)).collect::<Vec<_>>().into()
    }

    /// Java `getUnconnectedSet(netNumber)`.
    pub fn unconnected_set(&self, key: ItemKey, net_number: NetNo) -> ItemSet {
        let this = self.item(key);
        let mut result = ItemSet::new();
        if net_number > 0 && !this.contains_net(net_number) {
            return result;
        }
        if net_number > 0 {
            for k in self.get_connectable_items(net_number) {
                result.insert(self.item(k).id(), k);
            }
        } else {
            for &n in &this.net_numbers {
                for k in self.get_connectable_items(n) {
                    result.insert(self.item(k).id(), k);
                }
            }
        }
        result.remove_all(&self.connected_set(key, net_number, false));
        result
    }

    /// Java `getConnectionItems(stopOption)`: all traces and vias from this item until the next
    /// fork or terminal item.
    pub fn get_connection_items(&self, key: ItemKey, stop_option: StopConnectionOption) -> ItemSet {
        let this = self.item(key);
        let contacts = self.normal_contacts(key);
        let mut result = ItemSet::new();
        if this.is_routable() {
            result.insert(this.id(), key);
        }
        for start_contact in contacts.iter() {
            let mut current = start_contact;
            let Some(mut prev_contact_point) = self.normal_contact_point(key, current) else {
                // no unique contact point
                continue;
            };
            let mut prev_contact_layer = this.first_common_layer(self.item(current), self);
            if this.is_trace() {
                // Check, that there is only 1 contact at this location.
                let check_contacts = self.trace_normal_contacts_at(key, &prev_contact_point, false);
                if check_contacts.len() != 1 {
                    continue;
                }
            }
            // Search from current along the contacts until the next fork or nonroute item.
            loop {
                let ci = self.item(current);
                if !ci.is_routable() {
                    break;
                }
                // fastroute: around a closed ring of routable items without a fork the Java loop
                // never ends; stop where it started
                if result.contains(ci.id()) {
                    break;
                }
                if ci.is_via() {
                    if stop_option == StopConnectionOption::Via {
                        break;
                    }
                    if stop_option == StopConnectionOption::FanoutVia && self.is_fanout_via(current, Some(&result)) {
                        break;
                    }
                }
                result.insert(ci.id(), current);
                let current_contacts = self.normal_contacts(current);
                let mut next_contact: Option<(ItemKey, Point, LayerNo)> = None;
                let mut fork_found = false;
                for tmp in current_contacts.iter() {
                    let tmp_layer = ci.first_common_layer(self.item(tmp), self);
                    if tmp_layer >= 0 {
                        let Some(tmp_point) = self.normal_contact_point(current, tmp) else {
                            // no unique contact point
                            fork_found = true;
                            break;
                        };
                        if prev_contact_layer != tmp_layer || prev_contact_point != tmp_point {
                            if next_contact.is_some() {
                                // second new contact found
                                fork_found = true;
                                break;
                            }
                            next_contact = Some((tmp, tmp_point, tmp_layer));
                        }
                    }
                }
                match next_contact {
                    Some((next, point, layer)) if !fork_found => {
                        current = next;
                        prev_contact_point = point;
                        prev_contact_layer = layer;
                    }
                    _ => break,
                }
            }
        }
        result
    }

    /// Java `isTail()`: a trace not contacted at one end, or a via with contacts on at most one
    /// layer (range).
    pub fn is_tail(&self, key: ItemKey) -> bool {
        let this = self.item(key);
        match &this.kind {
            ItemKind::Trace(_) => self.trace_start_contacts(key).is_empty() || self.trace_end_contacts(key).is_empty(),
            ItemKind::Via(_) => {
                let contacts = self.normal_contacts(key);
                if contacts.len() <= 1 {
                    return true;
                }
                let mut it = contacts.iter();
                let first = self.item(it.next().unwrap());
                let (ff, fl) = (first.first_layer(self), first.last_layer(self));
                for k in it {
                    let c = self.item(k);
                    if c.first_layer(self) != ff || c.last_layer(self) != fl {
                        return false;
                    }
                }
                true
            }
            _ => false,
        }
    }

    /// Java `Trace.isOverlap()`: the trace is connected to the same object at both ends.
    pub fn is_overlap(&self, key: ItemKey) -> bool {
        if !self.item(key).is_trace() {
            return false;
        }
        !self.trace_start_contacts(key).is_disjoint(&self.trace_end_contacts(key))
    }

    /// Java `Trace.isCycle()`: the trace can be reached by other items via more than one path.
    pub fn is_cycle(&self, key: ItemKey) -> bool {
        if self.is_overlap(key) {
            return true;
        }
        let this = self.item(key);
        let start_contacts = self.trace_start_contacts(key);
        // a cycle exists if through expanding the start contact we reach this trace again via
        // an end contact
        let mut visited = start_contacts.clone();
        let mut ignore_areas = false;
        if let Some(&n) = this.net_numbers.first() {
            if let Some(net) = self.rules.nets.get(n) {
                ignore_areas = self.rules.net_classes[net.get_net_class()].get_ignore_cycles_with_areas();
            }
        }
        for contact in start_contacts.iter() {
            if self.is_cycle_recu(contact, &mut visited, key, key, ignore_areas) {
                return true;
            }
        }
        false
    }

    /// Java `Item.isCycleRecu(visitedItems, searchItem, comeFromItem, ignoreAreas)` (iterative).
    fn is_cycle_recu(&self, start: ItemKey, visited: &mut ItemSet, search: ItemKey, come_from: ItemKey, ignore_areas: bool) -> bool {
        struct Frame {
            item: ItemKey,
            come_from: ItemKey,
            contacts: Vec<ItemKey>,
            pos: usize,
        }
        let make_frame = |item: ItemKey, come_from: ItemKey| -> Option<Frame> {
            if ignore_areas && self.item(item).is_conduction_area() {
                return None;
            }
            Some(Frame { item, come_from, contacts: self.normal_contacts(item).iter().collect(), pos: 0 })
        };
        let Some(first) = make_frame(start, come_from) else {
            return false;
        };
        let mut stack = vec![first];
        while let Some(frame) = stack.last_mut() {
            if frame.pos >= frame.contacts.len() {
                stack.pop();
                continue;
            }
            let contact = frame.contacts[frame.pos];
            frame.pos += 1;
            if contact == frame.come_from {
                continue;
            }
            if contact == search {
                return true;
            }
            let from = frame.item;
            if visited.insert(self.item(contact).id(), contact) {
                if let Some(f) = make_frame(contact, from) {
                    stack.push(f);
                }
            }
        }
        false
    }

    /// Java `isFanoutVia(ignoreItems)`.
    pub fn is_fanout_via(&self, key: ItemKey, ignore_items: Option<&ItemSet>) -> bool {
        let is_single_contact_smd_pin = |k: ItemKey| -> bool {
            let c = self.item(k);
            c.is_pin() && c.first_layer(self) == c.last_layer(self) && self.normal_contacts(k).len() <= 1
        };
        for contact in self.normal_contacts(key).iter() {
            if is_single_contact_smd_pin(contact) {
                return true;
            }
            let c = self.item(contact);
            if let Some(t) = c.as_trace() {
                if let Some(ignore) = ignore_items {
                    if ignore.contains(c.id()) {
                        continue;
                    }
                }
                if t.length() >= PROTECT_FANOUT_LENGTH * t.half_width as f64 {
                    continue;
                }
                for tmp in self.normal_contacts(contact).iter() {
                    if is_single_contact_smd_pin(tmp) {
                        return true;
                    }
                    let ti = self.item(tmp);
                    if let Some(tt) = ti.as_trace() {
                        if ti.fixed_state() == FixedState::ShoveFixed && tt.corner_count() == 2 {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }

    /// Java `getRatsnestCorners()`.
    pub fn ratsnest_corners(&self, key: ItemKey) -> Vec<Point> {
        let this = self.item(key);
        match &this.kind {
            ItemKind::Pin(_) | ItemKind::Via(_) => vec![this.center(self)],
            ItemKind::Trace(_) => {
                let mut result = Vec::new();
                if self.trace_start_contacts(key).is_empty() {
                    result.push(this.first_corner());
                }
                if self.trace_end_contacts(key).is_empty() {
                    result.push(this.last_corner());
                }
                result
            }
            ItemKind::ConductionArea(c) => {
                let corners: Vec<FloatPoint> = c.area.get_area(self).corner_approx_arr();
                corners.iter().map(|p| Point::Int(p.round())).collect()
            }
            _ => Vec::new(),
        }
    }

    /// Java `Connectable.getTraceConnectionShape(tree, index)`.
    pub fn trace_connection_shape(&self, t: usize, key: ItemKey, index: i32) -> Option<TileShape> {
        let this = self.item(key);
        match &this.kind {
            ItemKind::Pin(_) | ItemKind::Via(_) => Some(TileShape::IntBox(TileShape::get_instance_point(&this.center(self)))),
            ItemKind::Trace(tr) => {
                if index < 0 || index >= tr.tile_shape_count() {
                    log::warn!("PolylineTrace.get_trace_connection_shape index out of range");
                    return None;
                }
                let segment = fr_geom::LineSegment::from_polyline(&tr.polyline, index + 1)?;
                Some(TileShape::Simplex(segment.to_simplex()).simplify())
            }
            ItemKind::ConductionArea(_) => {
                if index < 0 || index >= self.tree_shape_count(t, key) {
                    log::warn!("ConductionArea.get_trace_connection_shape index out of range");
                    return None;
                }
                self.tree_shape(t, key, index)
            }
            _ => None,
        }
    }

    /// Java `getConnectedSets(netNumber)`.
    pub fn get_connected_sets(&self, net_number: NetNo) -> Vec<ItemSet> {
        let mut result = Vec::new();
        if net_number <= 0 {
            return result;
        }
        let mut items_to_handle = ItemSet::new();
        for k in self.get_connectable_items(net_number) {
            items_to_handle.insert(self.item(k).id(), k);
        }
        while let Some(current) = items_to_handle.first() {
            let next_set = self.connected_set(current, net_number, false);
            items_to_handle.remove_all(&next_set);
            result.push(next_set);
        }
        result
    }
}

/// Java `Trace.normalContactPoint(Trace other)` with `this`.
fn trace_trace_contact_point(this: &Item, other: &Item) -> Option<Point> {
    if this.trace().layer != other.trace().layer {
        return None;
    }
    let tf = this.first_corner();
    let tl = this.last_corner();
    let of = other.first_corner();
    let ol = other.last_corner();
    let at_first = tf == of || tf == ol;
    let at_last = tl == of || tl == ol;
    if !(at_first || at_last) || (at_first && at_last) {
        None
    } else if at_first {
        Some(tf)
    } else {
        Some(tl)
    }
}

impl BasicBoard {
    /// fastroute: trace ends that lie in the copper of a pin or via of their net but not at its
    /// center (KiCad's Specctra export rounds them, e.g. 0.5 um off the pad center) are not
    /// connected in the contact model, so the pin stays "unrouted" although the maze search
    /// regards it as reached. Adds a short trace from each such end to the center (same layer,
    /// width, nets, clearance class and fixed state; inside the convex pad shape). Returns the
    /// number of traces added.
    pub fn bridge_trace_ends_to_drill_centers(&mut self) -> usize {
        let mut bridges: Vec<(Point, Point, LayerNo, i32, Vec<NetNo>, crate::ids::ClearanceClassNo, FixedState)> = Vec::new();
        for key in self.get_traces() {
            let t = self.item(key);
            let layer = t.trace().layer;
            for end in [t.first_corner(), t.last_corner()] {
                let contacts = self.trace_normal_contacts_at(key, &end, false);
                if contacts.iter().any(|c| matches!(self.item(c).kind, ItemKind::Pin(_) | ItemKind::Via(_))) {
                    continue;
                }
                for o in self.overlapping_objects(&point_shape(&end), layer) {
                    let TreeObject::Item { key: other_key, .. } = o else { continue };
                    let other = self.item(other_key);
                    if !matches!(other.kind, ItemKind::Pin(_) | ItemKind::Via(_)) || !other.shares_net(t) {
                        continue;
                    }
                    let center = other.center(self);
                    if center == end {
                        continue;
                    }
                    // the center must be in the copper on this layer as well (convex shape)
                    let index = layer - other.first_layer(self);
                    let Some(shape) = self.tile_shape(other_key, index) else { continue };
                    if shape.contains(&center) && shape.contains(&end) {
                        // The bridge stays inside the pad's copper: a capsule lies in a convex shape when
                        // both its end discs do, so its half-width is at most the room round each end.
                        // At the trace's own half-width a trace wider than the pad put new copper outside
                        // it, within clearance of other nets (parisxmas/fastroute#3).
                        let room = shape.border_distance(&center.to_float()).min(shape.border_distance(&end.to_float()));
                        let half_width = t.trace().half_width.min(room.floor() as i32);
                        if half_width > 0 {
                            bridges.push((end, center, layer, half_width, t.net_numbers().to_vec(), t.clearance_class, t.fixed_state()));
                        }
                        break;
                    }
                }
            }
        }
        let n = bridges.len();
        for (a, b, layer, half_width, nets, cl, fixed) in bridges {
            let polyline = fr_geom::Polyline::from_points(&[a, b]);
            self.insert_trace(polyline, layer, half_width, &nets, cl, fixed);
        }
        n
    }
}
