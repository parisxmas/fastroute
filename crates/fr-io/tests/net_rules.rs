//! Rules written inside a net scope. A clearance there constrains one net, which the board
//! model cannot express without giving it a class of its own; the reference loader drops it, so
//! this loader drops it too and says which net it ignored.

use fr_io::load_bytes;

#[test]
fn per_net_clearance_rule_is_reported() {
    let d = load_bytes(include_bytes!("../testdata/dsn/synth_net_clearance.dsn")).unwrap();
    let warned: Vec<&String> = d
        .warnings
        .iter()
        .filter(|w| w.contains("clearance rule of net 'B'"))
        .collect();
    assert_eq!(warned.len(), 1, "{:?}", d.warnings);
    // The net keeps the class it had: the numbering of the clearance classes stays the one the
    // reference loader produces.
    let class_a = d.rules.nets.get_by_name("A", 1).unwrap().get_net_class();
    let class_b = d.rules.nets.get_by_name("B", 1).unwrap().get_net_class();
    assert_eq!(class_a, class_b);
    let ca = d.rules.net_classes[class_a].get_trace_clearance_class();
    assert_eq!(d.rules.clearance_matrix.get_value(ca, ca, 0, false), 2000);
}
