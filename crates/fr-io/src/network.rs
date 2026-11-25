//! The network scope (Java `io.specctra.parser.Network.readScope` and helpers): nets and
//! subnets, via infos, via rules, net classes, class-class rules; then the components
//! ([`crate::components`]) and logical parts ([`crate::part_library`]).

use std::collections::BTreeSet;

use fr_dsn::model::{ClassClass, Net as DsnNet, NetClass as DsnNetClass, PinRef, Rule};
use fr_engine::ids::{ClearanceClassNo, LayerNo};
use fr_engine::rules::{BoardRules, ClearanceMatrix, ItemClass, NetClassId, ViaInfo, ViaRule};
use fr_engine::structure::CoordinateTransform;

use crate::error::{npe, LResult, LoadError};
use crate::library::clean_padstack_name;
use crate::loader::{Board, Loader};
use crate::netlist::{NetId, PinKey};
use crate::structure::{contains_wire_clearance_pair, round_i32};

/// Java `KiCadNetClassNames.isKiCadDefaultNetClassName`.
pub fn is_kicad_default_net_class_name(name: &str) -> bool {
    !name.is_empty()
        && (name.eq_ignore_ascii_case("default") || name.eq_ignore_ascii_case("kicad_default"))
}

/// Java `String.split("_")` (trailing empty strings removed).
fn java_split_underscore(s: &str) -> Vec<&str> {
    let mut v: Vec<&str> = s.split('_').collect();
    while v.len() > 1 && v.last().is_some_and(|x| x.is_empty()) {
        v.pop();
    }
    if v.len() == 1 && v[0].is_empty() && !s.is_empty() {
        // "___" splits into nothing in Java
        v.clear();
    }
    v
}

fn set_value(cm: &mut ClearanceMatrix, i: i32, j: i32, layer: LayerNo, value: i32) {
    if layer < 0 {
        cm.set_value_all_layers(i, j, value);
    } else {
        cm.set_value(i, j, layer, value);
    }
}

/// Java `Network.getClearanceClass`: the class `<netclass>-<item>` (or the net class itself
/// for `wire`), created from the net class row if missing.
fn get_clearance_class(
    rules: &mut BoardRules,
    net_class: NetClassId,
    item_class_name: &str,
) -> ClearanceClassNo {
    let net_class_name = rules.net_classes[net_class].get_name().to_string();
    let new_name = if item_class_name == "wire" {
        net_class_name.clone()
    } else {
        format!("{net_class_name}-{item_class_name}")
    };
    let cm = &mut rules.clearance_matrix;
    let found = cm.get_no(&new_name);
    if found >= 0 {
        return found;
    }
    cm.append_class(&new_name);
    let result = cm.get_no(&new_name);
    let net_class_no = cm.get_no(&net_class_name);
    if net_class_no < 0 || result < 0 {
        log::warn!("Network.get_clearance_class: clearance class not found at '{net_class_name}'");
        return result;
    }
    for i in 1..cm.get_class_count() {
        for j in 0..cm.get_layer_count() {
            let v = cm.get_value(net_class_no, i, j, false);
            cm.set_value(result, i, j, v);
            cm.set_value(i, result, j, v);
        }
    }
    let ic = match item_class_name {
        "via" => Some(ItemClass::Via),
        "pin" => Some(ItemClass::Pin),
        "smd" => Some(ItemClass::Smd),
        "area" => Some(ItemClass::Area),
        _ => None,
    };
    if let Some(ic) = ic {
        rules.net_classes[net_class]
            .default_item_clearance_classes
            .set(ic, result);
    }
    result
}

/// Java `Network.addClearanceRule`.
fn add_clearance_rule(
    rules: &mut BoardRules,
    net_class: NetClassId,
    value: f64,
    pairs: &[String],
    layer: LayerNo,
    ct: &CoordinateTransform,
) {
    let clearance = round_i32(ct.dsn_to_board(value));
    let class_name = rules.net_classes[net_class].get_name().to_string();
    let mut class_no = rules.clearance_matrix.get_no(&class_name);
    if class_no < 0 {
        let cm = &mut rules.clearance_matrix;
        cm.append_class(&class_name);
        class_no = cm.get_no(&class_name);
        for i in 1..cm.get_class_count() {
            for j in 0..cm.get_layer_count() {
                let v = cm.get_value(class_no, i, j, false).max(clearance);
                cm.set_value(class_no, i, j, v);
                cm.set_value(i, class_no, j, v);
            }
        }
        rules.net_classes[net_class]
            .default_item_clearance_classes
            .set_all(class_no);
    }
    rules.net_classes[net_class].set_trace_clearance_class(class_no);
    if pairs.is_empty() {
        set_value(
            &mut rules.clearance_matrix,
            class_no,
            class_no,
            layer,
            clearance,
        );
        return;
    }
    if contains_wire_clearance_pair(pairs) {
        for n in ["via", "smd", "pin", "area"] {
            get_clearance_class(rules, net_class, n);
        }
    }
    for current in pairs {
        let pair = java_split_underscore(current);
        if pair.len() != 2 {
            continue;
        }
        let first = get_clearance_class(rules, net_class, pair[0]);
        let second = get_clearance_class(rules, net_class, pair[1]);
        set_value(&mut rules.clearance_matrix, first, second, layer, clearance);
        set_value(&mut rules.clearance_matrix, second, first, layer, clearance);
    }
}

/// Java `Network.addMixedClearanceRule`.
fn add_mixed_clearance_rule(
    rules: &mut BoardRules,
    first_class: NetClassId,
    second_class: NetClassId,
    value: f64,
    pairs: &[String],
    layer: LayerNo,
    ct: &CoordinateTransform,
) {
    let clearance = round_i32(ct.dsn_to_board(value));
    let class_no = |rules: &mut BoardRules, c: NetClassId| {
        let name = rules.net_classes[c].get_name().to_string();
        let mut no = rules.clearance_matrix.get_no(&name);
        if no < 0 {
            rules.clearance_matrix.append_class(&name);
            no = rules.clearance_matrix.get_no(&name);
        }
        no
    };
    let first_no = class_no(rules, first_class);
    let second_no = class_no(rules, second_class);
    if pairs.is_empty() {
        set_value(
            &mut rules.clearance_matrix,
            first_no,
            second_no,
            layer,
            clearance,
        );
        set_value(
            &mut rules.clearance_matrix,
            second_no,
            first_no,
            layer,
            clearance,
        );
        return;
    }
    for current in pairs {
        let pair = java_split_underscore(current);
        if pair.len() != 2 {
            continue;
        }
        for i in 0..2 {
            let (a, b) = if i == 0 {
                (
                    get_clearance_class(rules, first_class, pair[0]),
                    get_clearance_class(rules, second_class, pair[1]),
                )
            } else {
                (
                    get_clearance_class(rules, second_class, pair[0]),
                    get_clearance_class(rules, first_class, pair[1]),
                )
            };
            set_value(&mut rules.clearance_matrix, a, b, layer, clearance);
            set_value(&mut rules.clearance_matrix, b, a, layer, clearance);
        }
    }
}

/// Java `Network.createDefaultViaInfos`.
fn create_default_via_infos(board: &mut Board, net_class: NetClassId, attach_allowed: bool) {
    let rules = &mut board.rules;
    let cl = rules.net_classes[net_class]
        .default_item_clearance_classes
        .get(ItemClass::Via);
    let is_default = net_class == rules.get_default_net_class();
    let class_name = rules.net_classes[net_class].get_name().to_string();
    for i in 0..board.library.via_padstack_count() {
        let p = board.library.get_via_padstack(i).expect("via padstack");
        let via_attach = attach_allowed && p.attach_allowed;
        let name = if is_default {
            p.name.clone()
        } else {
            format!("{}-{}", p.name, class_name)
        };
        rules
            .via_infos
            .add(ViaInfo::new(name, p.id, cl, via_attach));
    }
}

/// Java `Network.addViaRule`: true if all vias of the rule were found.
fn add_via_rule(rules: &mut BoardRules, names: &[String]) -> bool {
    let rule_name = &names[0];
    let existing = rules.get_via_rule(rule_name);
    let mut rule = ViaRule::new(rule_name.clone());
    let mut ok = true;
    for n in &names[1..] {
        match rules.via_infos.get_by_name(n) {
            Some(v) => rule.append_via(v),
            None => {
                log::warn!("Network.insert_via_rules: viaInfo not found");
                ok = false;
            }
        }
    }
    if ok {
        if let Some(e) = existing {
            rules.via_rules.remove(e);
        }
        rules.via_rules.add(rule);
    }
    ok
}

/// Java `Network.createOrderedSubnets`.
fn create_ordered_subnets(pins: &[PinKey]) -> Vec<BTreeSet<PinKey>> {
    pins.windows(2)
        .map(|w| w.iter().cloned().collect::<BTreeSet<_>>())
        .collect()
}

/// Java `Network.readNetPins` reads the pin name after the hyphen with `nextString`, so a
/// quoted pin name (`U12-"D-"`) loses its quotes. The typed model keeps an unquoted atom with
/// inner quotes as is (`"D-"`); strip them here (workaround, belongs in fr-dsn's `pin_ref`).
fn pin_key(p: &PinRef) -> PinKey {
    let pin = match p.pin.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
        Some(inner) if p.pin.len() >= 2 && !inner.contains('"') => inner.to_string(),
        _ => p.pin.clone(),
    };
    PinKey {
        component: p.component.clone(),
        pin,
    }
}

impl Loader<'_> {
    pub(crate) fn read_network(&mut self) -> LResult<()> {
        let dsn = self.dsn;
        let nw = &dsn.network;
        if self.board.is_none() {
            return Err(npe("Network.readScope: board is null"));
        }

        // Scope loop: nets are evaluated immediately, via infos are resolved immediately
        // (adding their padstacks to the via padstack list), classes are collected.
        for net in &nw.nets {
            self.read_net(net)?;
        }
        let mut via_infos: Vec<ViaInfo> = Vec::new();
        for v in &nw.vias {
            via_infos.push(self.read_via_info(v)?);
        }

        // Via padstack names: structure `(via ...)` plus the `use_via` lists of the classes.
        // Without a structure via scope, Java aliases the first class's use_via list and
        // appends the others to it (mutating that class's use_via).
        let mut use_via: Vec<Vec<String>> = nw.classes.iter().map(|c| c.use_via.clone()).collect();
        let mut names: Option<Vec<String>> = dsn.structure.via_padstacks.clone();
        let mut alias: Option<usize> = None;
        for i in 0..use_via.len() {
            if let Some(a) = alias {
                let add = use_via[i].clone();
                use_via[a].extend(add);
            } else if let Some(n) = names.as_mut() {
                n.extend(use_via[i].iter().cloned());
            } else {
                alias = Some(i);
            }
        }
        if let Some(a) = alias {
            names = Some(use_via[a].clone());
        }

        let via_at_smd = self.via_at_smd;
        let board = self.board()?;
        if let Some(names) = names {
            if !board.library_read {
                return Err(npe("Network.readScope: library padstacks are null"));
            }
            let mut found = Vec::new();
            for n in &names {
                let cleaned = clean_padstack_name(n);
                match board.library.padstacks.get_by_name(&cleaned) {
                    Some(p) => found.push(p.id),
                    None => {
                        log::warn!("Library.read_scope: via padstack with name '{n}' not found")
                    }
                }
            }
            board.library.set_via_padstacks(found);
        }

        // insertViaInfos
        if !via_infos.is_empty() {
            for v in via_infos {
                board.rules.via_infos.add(v);
            }
        } else {
            let d = board.rules.get_default_net_class();
            create_default_via_infos(board, d, via_at_smd);
        }
        // insertViaRules
        let mut rule_found = false;
        for r in &nw.via_rules {
            if r.len() < 2 {
                continue;
            }
            if add_via_rule(&mut board.rules, r) {
                rule_found = true;
            }
        }
        if !rule_found {
            let d = board.rules.get_default_net_class();
            board
                .rules
                .create_default_via_rule(d, "default", &board.library.padstacks);
        }
        let default_rule = board.rules.get_default_via_rule();
        for i in 0..board.rules.net_classes.count() {
            let c = board.rules.net_classes.get(i);
            board.rules.net_classes[c].set_via_rule(default_rule);
        }

        for (i, c) in nw.classes.iter().enumerate() {
            self.insert_net_class(c, &use_via[i])?;
        }
        for cc in &nw.class_classes {
            self.insert_class_pairs(cc)?;
        }
        self.insert_components()?;
        self.insert_logical_parts()?;
        Ok(())
    }

    /// Java `Network.readNetScope` (after parsing).
    fn read_net(&mut self, net: &DsnNet) -> LResult<()> {
        let contains_plane = self.layers().contains_plane(&net.name);
        let ct = self.board()?.transform;
        let pins: Vec<PinKey> = net.pins.iter().map(pin_key).collect();
        let subnet_pin_lists: Vec<BTreeSet<PinKey>> = if !net.fromtos.is_empty() {
            net.fromtos
                .iter()
                .map(|f| f.iter().map(pin_key).collect())
                .collect()
        } else if net.ordered {
            create_ordered_subnets(&pins)
        } else {
            vec![pins.into_iter().collect()]
        };
        let mut subnet = net.subnet;
        for pin_list in subnet_pin_lists {
            let id = NetId {
                name: net.name.clone(),
                subnet,
            };
            if !self.netlist.contains(&id) && self.netlist.add_net(id.clone()) {
                self.board()?
                    .rules
                    .add_net(&net.name, subnet, contains_plane);
            }
            self.netlist.set_pins(&id, pin_list);
            if !net.rules.is_empty() {
                let mut ignored_rules = Vec::new();
                let board = self.board()?;
                let Some(board_net) = board.rules.nets.get_by_name(&net.name, subnet) else {
                    log::warn!("Network.read_net_scope: board net not found");
                    return Ok(());
                };
                let net_no = board_net.net_number;
                for r in &net.rules {
                    match r {
                        Rule::Width(w) => {
                            let d = board.rules.get_default_net_class();
                            let hw = round_i32(ct.dsn_to_board(*w) / 2.0);
                            let (cl, vr) = {
                                let dc = &board.rules.net_classes[d];
                                (dc.get_trace_clearance_class(), dc.get_via_rule())
                            };
                            let class = match board.rules.net_classes.find(hw, cl, vr) {
                                Some(c) => c,
                                None => board.rules.get_new_net_class(),
                            };
                            board.rules.net_classes[class].set_trace_half_width(hw);
                            board
                                .rules
                                .nets
                                .get_mut(net_no)
                                .expect("net")
                                .set_class(class);
                        }
                        Rule::Clearance { .. } => {
                            // Applying it needs a class of the net's own (a clearance lives on
                            // the net class), which changes the clearance class numbering the
                            // reference loader produces, so it stays dropped as there.
                            ignored_rules.push(format!(
                                "Network.read_net_scope: clearance rule of net '{}' not applied; give the net its own class",
                                net.name
                            ));
                        }
                    }
                }
                for msg in ignored_rules {
                    self.warn(msg);
                }
            }
            subnet = subnet.wrapping_add(1);
        }
        Ok(())
    }

    /// Java `Network.readViaInfo` (evaluated during the scope loop).
    fn read_via_info(&mut self, v: &fr_dsn::model::ViaInfo) -> LResult<ViaInfo> {
        let board = self.board()?;
        let padstack = match board.library.get_via_padstack_by_name(&v.padstack) {
            Some(p) => p.id,
            None => {
                if !board.library_read {
                    return Err(npe("Network.readViaInfo: library padstacks are null"));
                }
                let Some(p) = board.library.padstacks.get_by_name(&v.padstack) else {
                    return Err(LoadError::ParseError(format!(
                        "Network.read_via_info: padstack '{}' not found",
                        v.padstack
                    )));
                };
                let id = p.id;
                board.library.add_via_padstack(id);
                id
            }
        };
        let mut cl = board.rules.clearance_matrix.get_no(&v.clearance_class);
        if cl < 0 {
            cl = BoardRules::default_clearance_class();
        }
        Ok(ViaInfo::new(v.name.clone(), padstack, cl, v.attach))
    }

    /// Java `Network.insertNetClass`.
    fn insert_net_class(&mut self, nc: &DsnNetClass, use_via: &[String]) -> LResult<()> {
        let via_at_smd = self.via_at_smd;
        let pls = self.parser_layers.clone();
        let board = self.board()?;
        let ct = board.transform;
        let rules = &mut board.rules;
        let class = if is_kicad_default_net_class_name(&nc.name) {
            rules.get_default_net_class()
        } else {
            rules.append_net_class(&nc.name)
        };
        if let Some(cc) = &nc.clearance_class {
            let no = rules.clearance_matrix.get_no(cc);
            if no >= 0 {
                rules.net_classes[class].set_trace_clearance_class(no);
            } else {
                log::warn!("Network.insert_net_class: clearance class not found");
            }
        }
        if let Some(vr) = &nc.via_rule {
            match rules.get_via_rule(vr) {
                Some(r) => rules.net_classes[class].set_via_rule(Some(r)),
                None => log::warn!("Network.insert_net_class: via rule not found"),
            }
        }
        if nc.max_trace_length > 0.0 {
            rules.net_classes[class].set_maximum_trace_length(ct.dsn_to_board(nc.max_trace_length));
        }
        if nc.min_trace_length > 0.0 {
            rules.net_classes[class].set_minimum_trace_length(ct.dsn_to_board(nc.min_trace_length));
        }
        for net_name in &nc.nets {
            let nos: Vec<i32> = rules
                .nets
                .get_all_by_name(net_name)
                .iter()
                .map(|n| n.net_number)
                .collect();
            for no in nos {
                rules.nets.get_mut(no).expect("net").set_class(class);
            }
        }
        let mut clearance_rule_found = false;
        for r in &nc.rules {
            match r {
                Rule::Width(w) => {
                    let hw = round_i32(ct.dsn_to_board(*w / 2.0));
                    rules.net_classes[class].set_trace_half_width(hw);
                }
                Rule::Clearance { value, class_pairs } => {
                    add_clearance_rule(rules, class, *value, class_pairs, -1, &ct);
                    clearance_rule_found = true;
                }
            }
        }
        let board_ls = fr_engine::structure::LayerStructure::clone(rules.layer_structure());
        for lr in &nc.layer_rules {
            for layer_name in &lr.layers {
                let layer = board_ls.get_no(layer_name);
                if layer < 0 {
                    log::warn!("Network.insert_net_class: layer not found");
                    continue;
                }
                for r in &lr.rules {
                    match r {
                        Rule::Width(w) => {
                            let hw = round_i32(ct.dsn_to_board(*w / 2.0));
                            rules.net_classes[class].set_trace_half_width_on_layer(layer, hw);
                        }
                        Rule::Clearance { value, class_pairs } => {
                            add_clearance_rule(rules, class, *value, class_pairs, layer, &ct);
                            clearance_rule_found = true;
                        }
                    }
                }
            }
        }
        rules.net_classes[class].set_pull_tight(nc.pull_tight);
        rules.net_classes[class].set_shove_fixed(nc.shove_fixed);
        let mut via_infos_created = false;
        let is_default = class == rules.get_default_net_class();
        if clearance_rule_found && !is_default {
            create_default_via_infos(board, class, via_at_smd);
            via_infos_created = true;
        }
        let rules = &mut board.rules;
        if !use_via.is_empty() {
            // Network.createViaRule
            let name = rules.net_classes[class].get_name().to_string();
            let mut rule = ViaRule::new(name);
            let default_via_cl = rules.net_classes[class]
                .default_item_clearance_classes
                .get(ItemClass::Via);
            for via_name in use_via {
                for i in 0..rules.via_infos.count() {
                    let vi = rules.via_infos.get(i);
                    let info = &rules.via_infos[vi];
                    if info.get_clearance_class_index() == default_via_cl {
                        let pname = &board
                            .library
                            .padstacks
                            .get(info.get_padstack())
                            .expect("padstack")
                            .name;
                        if pname == via_name {
                            rule.append_via(vi);
                        }
                    }
                }
            }
            let id = rules.via_rules.add(rule);
            rules.net_classes[class].set_via_rule(Some(id));
        } else if via_infos_created {
            let name = rules.net_classes[class].get_name().to_string();
            rules.create_default_via_rule(class, &name, &board.library.padstacks);
        }
        if !nc.use_layer.is_empty() {
            // Network.createActiveTraceLayers (parser layer structure)
            let n = pls.len() as LayerNo;
            let c = &mut rules.net_classes[class];
            for i in 0..n {
                c.set_active_routing_layer(i, false);
            }
            let pl = crate::shapes::ParserLayers(&pls);
            for l in &nc.use_layer {
                c.set_active_routing_layer(pl.get_no(l), true);
            }
            for i in 0..n {
                if !c.is_active_routing_layer(i) {
                    c.set_trace_half_width_on_layer(i, 0);
                }
            }
        }
        Ok(())
    }

    /// Java `Network.insertClassPairs` for one `class_class` scope. Java iterates the class
    /// names with a single shared iterator: the first class that is found is paired with all
    /// following names, which exhausts the iterator.
    fn insert_class_pairs(&mut self, cc: &ClassClass) -> LResult<()> {
        let board = self.board()?;
        let ct = board.transform;
        let rules = &mut board.rules;
        let resolve = |rules: &mut BoardRules, name: &str| -> Option<NetClassId> {
            if is_kicad_default_net_class_name(name) {
                Some(rules.get_default_net_class())
            } else {
                rules.net_classes.get_by_name(name)
            }
        };
        let mut it = cc.classes.iter();
        while let Some(first_name) = it.next() {
            let Some(first) = resolve(rules, first_name) else {
                log::warn!("Network.insert_class_pairs: first class not found");
                continue;
            };
            for second_name in it.by_ref() {
                let Some(second) = resolve(rules, second_name) else {
                    log::warn!("Network.insert_class_pairs: second class not found");
                    continue;
                };
                for r in &cc.rules {
                    match r {
                        Rule::Clearance { value, class_pairs } => add_mixed_clearance_rule(
                            rules,
                            first,
                            second,
                            *value,
                            class_pairs,
                            -1,
                            &ct,
                        ),
                        Rule::Width(_) => {
                            log::warn!("Network.insert_class_pair_info: unexpected rule")
                        }
                    }
                }
                let ls = rules.layer_structure().clone();
                for lr in &cc.layer_rules {
                    for layer_name in &lr.layers {
                        let layer = ls.get_no(layer_name);
                        if layer < 0 {
                            log::warn!("Network.insert_class_pair_info: layer not found");
                            continue;
                        }
                        for r in &lr.rules {
                            match r {
                                Rule::Clearance { value, class_pairs } => add_mixed_clearance_rule(
                                    rules,
                                    first,
                                    second,
                                    *value,
                                    class_pairs,
                                    layer,
                                    &ct,
                                ),
                                Rule::Width(_) => log::warn!(
                                    "Network.insert_class_pair_info: unexpected layer rule type"
                                ),
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split() {
        assert_eq!(java_split_underscore("a_b"), vec!["a", "b"]);
        assert_eq!(java_split_underscore("a_b_c").len(), 3);
        assert_eq!(java_split_underscore("a_"), vec!["a"]);
        assert_eq!(java_split_underscore("_b"), vec!["", "b"]);
        assert_eq!(java_split_underscore("").len(), 1);
        assert_eq!(java_split_underscore("__").len(), 0);
    }

    #[test]
    fn kicad_default() {
        assert!(is_kicad_default_net_class_name("Default"));
        assert!(is_kicad_default_net_class_name("kicad_default"));
        assert!(!is_kicad_default_net_class_name(""));
        assert!(!is_kicad_default_net_class_name("power"));
    }

    #[test]
    fn ordered_subnets() {
        let p = |c: &str| PinKey {
            component: c.into(),
            pin: "1".into(),
        };
        let s = create_ordered_subnets(&[p("U2"), p("U1"), p("U3")]);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].iter().next().unwrap().component, "U1");
        assert!(create_ordered_subnets(&[p("U1")]).is_empty());
    }
}

/// fastroute: applies the `class_class` clearances of a design also to the item clearance
/// classes of the two net classes. Java only sets the classes named after the net classes, but
/// pins and SMD pads of a class can use other clearance classes (KiCad's `kicad_default`: its
/// SMD pads use `smd`), so a class-pair clearance did not keep traces away from those pads.
/// Values only grow. Returns the number of matrix pairs changed.
pub fn extend_class_pair_clearances(board: &mut fr_engine::board::RoutingBoard, design: &fr_dsn::Dsn) -> usize {
    let ct = board.communication.coordinate_transform.clone();
    let rules = board.rules_mut();
    let resolve = |rules: &mut BoardRules, name: &str| -> Option<NetClassId> {
        if is_kicad_default_net_class_name(name) {
            Some(rules.get_default_net_class())
        } else {
            rules.net_classes.get_by_name(name)
        }
    };
    let item_classes = |rules: &BoardRules, c: NetClassId| -> BTreeSet<i32> {
        let d = &rules.net_classes[c].default_item_clearance_classes;
        let mut s: BTreeSet<i32> =
            [ItemClass::Trace, ItemClass::Via, ItemClass::Pin, ItemClass::Smd].iter().map(|&k| d.get(k)).filter(|&n| n > 0).collect();
        let own = rules.clearance_matrix.get_no(rules.net_classes[c].get_name());
        if own > 0 {
            s.insert(own);
        }
        s
    };
    let mut changed = 0;
    for cc in &design.network.class_classes {
        if cc.classes.len() != 2 {
            continue;
        }
        let (Some(a), Some(b)) = (resolve(rules, &cc.classes[0]), resolve(rules, &cc.classes[1])) else { continue };
        let (sa, sb) = (item_classes(rules, a), item_classes(rules, b));
        for r in &cc.rules {
            let Rule::Clearance { value, class_pairs } = r else { continue };
            if !class_pairs.is_empty() {
                continue;
            }
            let clearance = round_i32(ct.dsn_to_board(*value));
            for &i in &sa {
                for &j in &sb {
                    if rules.clearance_matrix.get_value(i, j, 0, false) < clearance {
                        rules.clearance_matrix.set_value_all_layers(i, j, clearance);
                        rules.clearance_matrix.set_value_all_layers(j, i, clearance);
                        changed += 1;
                    }
                }
            }
        }
    }
    changed
}
