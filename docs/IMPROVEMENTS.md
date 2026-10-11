# Improvements over Freerouting

fastroute started as a faithful port (`--parity` reproduces Freerouting's
deterministic results byte for byte). Changes that alter results are listed
here. The engine-side ones are active unless `--parity` is given
(`PipelineContext::enhancements`); the KiCad-side ones live in the plugin
(`integrations/kicad/plugins/core.py`).

## Engine

| Change | Why |
|---|---|
| A tree shape that cannot be built no longer aborts the connection | `ObstacleExpansionRoom: tree shape is null` panicked (Java throws) for a degenerate item shape, reported with partly routed boards whose existing tracks are unlocked; the shape is now skipped with one warning. |
| Footprints without a library id written as `""` in the session | Java writes an empty name, which KiCad's session import rejects. |
| Final tail removal also after the autorouter stopped itself | Freerouting skips `removeTails` when the stagnation rule stops the router, leaving unused fanout escapes (via + stub) on the board. KiCad reports them as dangling vias (50 on the lora board). |
| `router.min_trace_width_um` (option, unset by default) | Neck-down (at pins, fanout "micro neck-down", necked retry) never goes below this width; narrower candidates are raised to it. |
| The autorouter's own stop (stagnation, max passes) is withdrawn before the optimizer | Java keeps the stop request set, so every optimizer candidate fails to route and the optimizer gives up after its failure limit. |
| Optimizer also runs on partly routed boards | Java skips it whenever a connection is unrouted; the acceptance rules already reject candidates that add unrouted connections. |
| Greedy optimizer passes | Java evaluates every candidate but applies only the single best one per pass, so on larger boards a pass rarely clears the 2.5 % continuation threshold. After the winner, the other improving candidates are re-routed best-first on the current board and kept if the optimizer score improves without more unrouted connections or violations. This phase runs on one thread and clones the board per candidate, so it is limited to the pass's evaluation time (2–60 s) and ends after 10 rejected candidates in a row; unlimited, a 874-part 6-layer board spent ~7 min per pass in it (1258 candidates) and looked hung. |
| Optimizer re-routes only the ripped item's nets | Java's re-route runs full autorouter passes over every unrouted connection on the board for each candidate, retrying hopeless connections hundreds of times. |
| A failing connection rips its net only once | Java rips every unfixed trace/via of the net on each failure from the second one on. A connection that can never be routed (e.g. an enclosed GND pin) then tears down its whole net every other pass: lora oscillated between 12 and 19 unrouted. Unrouted after the change: bm05 10 → 7, complex_hierarchy 10 → 8, lora 12 → 11, StickHub 2 → 3. |
| Parallel multi-start routing (`--multi-start=N`, default 4) | If the autorouter leaves connections unrouted, it is rerun N−1 more times in parallel with shuffled (seeded) first-pass orders and the best board is kept (fewest unrouted, then violations, then router score). Boards that route completely pay nothing; others take about twice as long. Unrouted: bm04 1 → 0, bm05 7 → 2, CM5 8 → 7, StickHub 3 → 1. |
| Connections may end in a via into the net's plane | A found path that ends on the net's plane (an inactive power layer) is rejected as "some of their layers are disabled", so plane nets only reached their plane in the fanout stage and the rest stayed unrouted. On a 4-layer ESP32-S3 board with GND and 3V3 planes: 24 → 0 unrouted. |
| Parallel autorouting pass (`router.autorouter.max_threads`) | Freerouting routes the connections of a pass one after the other. fastroute routes up to 2 connections per thread at a time, each on a clone of the board as it was when the connection was started (preferring connections away from the ones in flight, never two of one net), and commits the results in start order by copying the changed traces/vias onto the board. A result is only copied if the items it replaced are unchanged and the copies have no new clearance violations; otherwise the connection is retried (twice) and then routed sequentially. Commit order and start decisions only depend on the commits, so results do not depend on thread timing (they do depend on the thread count). 6-layer test board (874 parts, 8 classes, planes): routing stage 1413 s → 508 s with 8 threads, 165 → 153 unrouted; failing connections (most of the later passes) run fully in parallel (pass time 163 s → 37 s). |
| Undo a bad autorouting pass | A pass that ends with far more unrouted items than the best pass so far (> best + max(20, 30 %)) is undone and the next pass starts from the best board (at most 3 times). Java keeps routing from the worse board: a 6-layer test board went from 291 to 552 unrouted in pass 2 when a failing connection ripped whole nets. |
| Slow-pass stagnation stop | When a pass takes 20 s or more, the autorouter stops once the last 3 passes reduced the unrouted items by less than 2 % (at least one). Java's rules only start after 8 passes, which is 80+ minutes at 10 minutes per pass. |
| Multi-start skipped after long runs | The parallel variants each take as long as the first run; after a first run longer than 10 minutes they are skipped. Variants also stop on a job stop (time limit, Ctrl+C) now. |
| No fanout on ignored net classes | Pins of classes in `router.autorouter.ignore_net_classes` get no fanout stubs (Java fans them out, so nets that must stay unrouted got 72 stubs on the test board). |
| Fanout stops when it only re-fans | A fanout pass that only fans out pins the previous pass fanned out as well (ripped again in between) ends the fanout stage; Java needed 3 identical passes. |
| Clearance shortfalls re-measured on exact geometry | Tile shapes bound round copper by octagons, up to 8.2 % of the radius past the circle; the measured clearance is smaller than the true one (0.06 mm beside a 1.5 mm power trace). On a 6-layer board whose fixed copper KiCad's DRC passes this produced 13 "pre-existing" violations. A reported shortfall is re-measured with vias as circles and trace segments as capsules (`drc/exact.rs`); pads keep their shape. The report only: routing keeps the conservative octagons. |
| A footprint's own pads are not judged against each other | Pad spacing inside a footprint is the part's; no router can change it, and KiCad does not report it. |
| Same-net via pairs by drill gap (`router.same_net_via_hole_to_hole_um`, option) | Freerouting holds two vias of one net to the copper clearance between different nets; a fab's rule for them is drill-to-drill. On the 6-layer board: 210 via pairs of a decoupling fanout at 0.45-0.6 mm pitch reported, 0 with the option at 250 um; an injected pair 0.20 mm drill-to-drill is still reported. Report only. |
| Clearance exempt areas (`router.clearance_exempt_areas`, option) | KiCad scopes a rule by rule area (a USB spacing rule not applied inside the USB pin field); a DSN cannot. Inside an entry's box, a pair with an item of the class is held to the entry's base clearance. 50 reported pairs on the 6-layer board, 0 with the option. Report only. |
| Optimizer skips connections that were already unrouted | A candidate re-routes the nets of the ripped item; connections of those nets that were unrouted when the optimizer started were re-tried for every candidate (each attempt seconds long on a board with hundreds of failing connections), without changing the comparison. They are skipped now. 6-layer test board: optimizer pass 897 s → 106 s with the same candidates and scores. |
| Optimizer time budget | Without `router.optimizer.timeout` the optimizer gets as long as the routing stage took (at least 60 s). The test board: whole run 1955 s → 644 s, same DRC result. |
| Unclamped optimizer score | The V2 score is clamped at 0; on bm01 the excess length/vias push it below 0, so no candidate could ever be accepted. The optimizer compares unclamped values (and the pass improvement relative to the magnitude). |
| Overlapping pins and vias are in contact | Freerouting connects two pins/vias only if their centers coincide, while the maze search regards touching copper as reached: pads of a net that touch, or a via in a pad off its center, stayed "unrouted" and were "routed" again in every pass without inserting anything. Copper overlapping on a common layer now counts as contact (`BasicBoard::set_overlap_contacts`). |
| Trace ends just off a pad center are joined to it | KiCad's Specctra export rounds fixed traces, which then end e.g. 0.5 µm next to the pad center; the contact rule (exact center) did not connect them. At load, every trace end inside the copper of a pin/via of its net but off its center gets a short trace to the center (inside the convex pad). lora_node: 11 → 2 unrouted; KiCad's DRC had already counted these as connected. (Treating any trace end in the copper as contact instead broke the pull-tight and tail logic: 12 → 80 unrouted.) |
| Connections that cannot be routed at all are skipped | After its second failure a connection is routed once alone on the board as loaded (pins, keepouts, fixed wiring); if it fails there too, no amount of rip-up can route it and later passes skip it instead of searching the whole board again. |
| Connections of ignored net classes are not work left | `autorouter.ignore_net_classes` connections are never routed but were counted as unrouted: multi-start ran three more routing runs on a finished board (628 s and 3.8 GB instead of 154 s and 2.8 GB on an 874-part board), and the rollback and stagnation thresholds were inflated by them. They are now left out of those decisions and reported separately ("315 of the 315 unrouted connections are in ignored net classes"; `--report`: `unrouted_ignored_classes`). |
| `router.autorouter.min_passes` | The stagnation rules (slow-pass rule, score rules) do not stop the autorouter before this many passes: a tester's 5.8-hour `--max-time` run ended after 1.6 hours because the last three passes had not improved (501 → 532 → 538 → 563 unrouted). |
| Fanout progress lines | A fanout pass over a big board takes many minutes (1092 s for 1266 pins on a 190 × 190 mm 6-layer board) with nothing in the log; now every 30 s: pins checked, fanned out, not routed. |
| Checkpoint after the fanout | The board after the fanout stage is written as the first checkpoint, so a run stopped (`--max-time`, Ctrl+C, live viewer) during its first autorouting pass leaves a session file behind. A fanout pass is not started after a stop request. |
| Stop watchdog | After a stop request a message every 30 s names the stage still finishing its current step, so a long step on a big board is not taken for a hang. |
| Fanout pins that take long are named | On the tester's board one pin's fanout took about 820 s of the 854 s pass (the per-pin limit bounds the maze search only). A pin taking 10 s or more gets a line with its component, pin, net, state and the time spent routing and checking. |
| Watchdog names the last progress | After a stop request the message also says the last committed connection ("pass 13, 310 of 690 connections committed") or the last checked fanout pin, so a step that does not finish can be told from a pass that is still being wound down. |
| Board history limited to 6 boards | Java's `BoardHistory` keeps up to 30 copies of the board for its restore rules; on a 23 000-item board that is gigabytes (the tester saw 0.6 → 3 GB over 12 passes). With the enhancements 6 copies are kept (the restore rules only ever look at the best few). |
| Score lines name the violations between fixed items | Clearance violations between two fixed items (imported wiring, with the plugin's 5 µm margin: 5 852 on the tester's board) cannot be fixed by any pass; the pass lines now say "N violations, M of them pre-existing between fixed items" instead of showing them as the router's. |
| An optimizer pass cut short keeps its best candidate | With the optimizer's time budget (as long as routing took) shorter than one pass, Java drops everything the pass found. The best candidate is applied, and the other improving ones get a 10 s greedy phase. |
| Progress inside long passes | Optimizer passes log their progress every 30 s; the autorouting progress lines show the current unrouted count. |
| A regressing pass is stopped early | When a parallel pass has more unrouted connections than the rollback threshold (best so far + max(20, 30 %)) at a progress line, it stops: it would be undone anyway (a tester lost 8–10 min per such pass). |
| Trace-only keepouts (`wire_keepout`) | Freerouting reads a Specctra `wire_keepout` as a full keepout, so KiCad rule areas that forbid tracks but allow vias also blocked vias. They block traces only now; vias may pass on boards with more than two layers (on two layers such a via cannot leave on the other side). 4-layer test: 22 → 19 unrouted, 59 → 21 violations. `--parity` keeps Freerouting's reading. |
| Fanout fallback vias of the net's clearance class | When a fanout fails with the net class's vias, Freerouting retries with the board's default vias, which carry the default clearance class: a via of a 0.2 mm class then sat 0.17 mm from other copper (KiCad DRC). Only fallback vias of the net's own clearance class are used. |
| Wiring stays inside the board outline | Freerouting exempts the traces of every net with a pin on or outside the outline (edge connectors, castellated pads, headers drawn across Edge.Cuts) from the outline entirely, so they could leave the board anywhere: on a Feather board with headers across the edge, VSYS ran up to 1.4 mm outside the board (4 traces). The area between the outline and the board's bounding box is now a keepout on all layers, with a window around each edge pin (its pad plus a trace width and clearance), so these nets still reach their edge pins but nothing else. Feather (200 s, same settings): 2 → 0 traces outside, 35 → 36 unrouted; a soldering-station board had a trace 7.7 mm outside, which now stays unrouted (2 → 3). |
| Fixed vias without a trace are zone stitching vias | A locked via that no trace touches (GND zone stitching, thermal vias; KiCad exports them as `(type fix)`) is an unrouted item of its net to Freerouting, which routes a trace to every one of them: a board with 184 GND stitching vias got 211 extra GND traces (413 → 791 mm). The zone that joins these vias is not on the board (the plugin strips the planes), so the stitching vias of a net are now treated as connected to each other: a pad routed to any one of them is connected to the zone and to every other pad that reached one. Same board: GND traces 120 / 390 mm without the vias, 104 / 228 mm with them, 0 unrouted, KiCad DRC clean. `--parity` keeps Freerouting's behaviour. |
| No endless walk around a ring of traces | `getConnectionItems` follows contacts until a fork; around a closed ring without a fork the Java loop never ends (it hung a run while the contact rules above were developed). The walk stops where it started. |

## Length matching (`--tune`, new)

Freerouting has no length matching. After routing and optimizing, fastroute can match groups
of nets (e.g. a memory bus): the target is the longest net of a group (or a given length);
shorter nets get serpentine meanders on their longest straight segments (both sides or
alternating, amplitudes 2 mm down to 0.45 mm, leg pitch 3× the width where it fits, else
width + clearance). Every meander is made on a board clone and kept only if a slightly wider
probe trace (+5 µm) has no clearance violation, so the result stays DRC-clean in KiCad too.
Lengths are trace lengths plus, for every via, the stackup height between the layers its
traces use (`--layer-heights`, which the plugin takes from the board's stackup): the same
length KiCad's DRC measures. Nets that have no room left are reported with the missing length.

The KiCad plugin writes the groups from the board's `.kicad_dru`: `skew` constraints (group
matched to its longest net) and `length (min …)` constraints, for conditions on
`A.NetClass == '…'` / `A.NetName == '…'` (wildcards) / `A.inDiffPair('…')` joined by `&&` / `||`.
A skew rule with `(within_diff_pairs)` makes each differential pair its own group (intra-pair
skew), as in KiCad; without it, all matching nets are matched to each other.

Intra-pair skew after the pair routing on the STM32H7 test board (KiCad DRC, rules 0.2 mm USB,
0.5 mm Ethernet): 1.8 / 2.7 / 14.5 mm before, 0.1 / 0.25 / 0.25 mm after, no DRC errors.

## Controlled impedance (new)

Freerouting has one trace width per net class and layer, taken from the DSN; it knows nothing
about impedance. fastroute adds two pieces:

- `integrations/kicad/plugins/impedance_cli.py` computes widths from the board's stackup
  (read from the `.kicad_pcb`; KiCad's default FR4 stackup if none is defined): outer layers
  as microstrip (Hammerstad–Jensen with Wadell's thickness correction), inner layers as
  stripline between the neighbouring layers (Wheeler, asymmetric case combined from two
  symmetric ones; within 0.3 % of Cohn's exact result), differential pairs with the IPC-2141
  coupling term. `--write` puts one `.kicad_dru` rule per class and layer (`track_width`
  min/opt/max, `diff_pair_gap` for pairs) into a marked block, so KiCad's DRC checks them.
- The plugin reads those per-layer widths back and writes them as DSN `layer_rule`s of the
  class, and passes `--no-neckdown-classes` for those classes: neck-down at pins (including
  the fanout micro neck-down) and the necked retry would break the width. Without this the
  test board had 30 KiCad `track_width` errors on the 50 Ω class; with it, 0.

Solder mask is not modelled (coated microstrips come out 2–4 Ω lower); widths should be
checked against the fabricator's stackup. Differential pairs get the width here and the
gap from the pair routing below.

## Differential pairs (`--pairs`, new)

Freerouting routes the two nets of a pair independently, so they rarely run side by side.
With `--pairs=FILE` fastroute routes the pairs first, on the empty board, and makes one net
follow the other:

1. Both nets are routed with the maze router.
2. The lead net's traces are chained into paths (imported boards have one trace per segment),
   oriented from the pin closest to the other net's pins, and offset sideways by
   `half width + half width + gap` towards the side of that pin (exact 45° geometry: the
   polyline's lines are translated, corners are their intersections).
3. The offset path is cut into 0.2 mm pieces; each piece is checked for clearance with a
   5 µm wider probe. Runs of free pieces (≥ 0.6 mm) become the follower's traces, the
   follower's old routing is removed, and the maze router joins the runs and the pins
   (fewer runs if it cannot join all of them).
4. The change is kept only if no connection is lost, the follower has no clearance
   violation and the score `coupled length − 0.5 × |length difference|` improves. Both
   directions (N following P, P following N) and both sides are tried.
5. The pair's traces and vias are fixed while the rest of the board is routed and optimized
   (like pairs a designer routes by hand first). If connections stay unrouted, the pairs are
   released and the board is routed once more: the autorouter then rips the pairs only where it
   must. Finally the coupling is tried again on the finished board, scored over all pairs
   (routing one pair may shove another away from its partner).

Test board (STM32H7, 4 layers, USB + 2 Ethernet pairs, through the plugin):

| | unrouted | KiCad DRC | coupled USB / ETH TX / ETH RX | time |
|---|---|---|---|---|
| no pairs | 0 | 0 | — | 193 s |
| pairs routed first, not held | 0 | 0 | 6 / 11 / 4 mm | 405 s |
| pairs held fixed throughout | 15 | 0 | 23 / 21 / 9 mm | 414 s |
| held, released if needed (default) | 0 | 0 | 14.5 / 11.3 / 13.8 mm of 28 / 32 / 24 | 507 s |

6. **Intra-pair skew.** After the coupling the shorter net of each pair gets meanders until
   the two lengths differ by at most the pair's tolerance (`skew=` in the pairs file,
   else `--pair-skew`, default 0.1 mm; 0 turns it off). The meanders go on the net's
   uncoupled parts first, longest segment first (near the pins, where the two nets run apart
   anyway); only what finds no room there goes on the coupled runs as small bumps (amplitude
   ≤ 0.45 mm), and the result says how much. Bumps next to the pins are at most 2 mm high.
   A difference above 5 mm is a detour of one net rather than pair geometry, and the log says
   so (the right fix is a shorter route for that net). Same board: USB −2.18 → −0.05 mm,
   ETH RX −0.80 → −0.05 mm, ETH TX +16.71 → +0.05 mm (flagged as a detour of TXN), all
   meanders off the coupled runs, 0 unrouted, 0 violations, KiCad DRC clean; the stage takes
   well under a second.

The gap is the pair clearance unless given (per layer as well). Limits: the pair is coupled
where there is room next to the lead net's path; ends whose pads are in the opposite order
need a crossing, which the router makes around a pad or with a via; branches (a USB-C
connector's two D+ pads) are coupled only along one path; the traces are length-matched
within the pair but not phase-matched at every bend
within the pair (the skew is logged).

## KiCad plugin

| Change | Why |
|---|---|
| Board minimum track width → `min_trace_width_um` and `neck_width_um` | The minimum is not in the DSN export. Neck-down stays enabled (fine-pitch pads need it) but stops at the minimum; failed insertions are retried with minimum-width traces (Freerouting's necked retry, off by default there). lora: 49 → 19 unrouted. |
| Board copper-to-edge clearance → `copper_to_edge_clearance_um` | Not in the DSN export; Freerouting's default is 250 µm. |
| Zone handling (`strip_planes`) | KiCad exports each zone as a plane covering its outline, so pads inside count as connected even if the refilled zone cuts them off. Pours on signal layers are removed and their nets routed with tracks. A power-type layer with one plane covering ≥ 50 % of the board is kept as a plane layer (pads connect through vias; no tracks on it). Other power-type layers become signal layers — Freerouting never routes on power layers, which made the 2-layer complex_hierarchy demo effectively single-layer. |
| Class-pair clearances also for pads | A DSN `class_class` clearance only set the clearance classes named after the two net classes; pads can use other clearance classes (KiCad's `kicad_default` SMD pads use `smd`), so traces kept only the plain clearance to them. fastroute applies the value to all item clearance classes (trace, via, pin, SMD) of both classes. |
| `.kicad_dru` net-class clearances → DSN `class_class` rules | KiCad's DSN export has one clearance per class; custom rules are lost. Rules whose condition only compares net classes (`A.NetClass == 'X' && B.NetClass != 'Y' ...`) with a minimum clearance constraint are written as class-pair clearances. |
| Copper texts → keepouts (glyph convex hulls) | Omitted by KiCad's DSN export. |
| Rule-area keepouts corrected | KiCad exports every rule area as a keepout, even areas that forbid nothing (KiCad 10's multichannel "auto-placement-area" regions). The router then blocked whole channels: multichannel_mixer went from 160 unrouted (Freerouting) to 0. Areas forbidding neither tracks nor vias are dropped; via-only areas become via keepouts. |
| Rule-area keepouts matched by layer and shape | A rule area that only carries a DRC rule (no restriction) is dropped from the DSN; it was matched by shape alone, so a blocking rule area of the same shape on the same layer went with it (a tester lost the keepouts of six channel areas and routed through them). Now keyed by (layer, shape), and a blocking area keeps exactly one keepout. |
| Zone refill after import | Stale fills overlapped the new tracks. |
| Clearance of two net classes = the larger one | KiCad applies the larger clearance of two net classes; Freerouting's DSN reader fills its matrix class by class, so the class read last decided a pair. Every pair of classes with different clearances is written as a `class_class` rule with the maximum. |
| `.kicad_dru` rules that are not applied are reported | One warning per routing rule (clearance, widths, length, disallow, hole clearance, ...) that is not carried over, with the reason (e.g. "its condition uses insideArea()", "the maximum length is not enforced"). Clearance rules with `A.Type`/`B.Type` terms are applied to all item types (stricter). |
| `hole_clearance` rules become keepout circles | Conditions on pads (`A.Type == 'Pad'`, `A.Hole_Size_X > 4mm`, `A.memberOfFootprint('X1?')`, net class/name, `&&`, `\|\|`, `!`) are evaluated; matching holes get a keepout of hole radius + clearance on all copper layers, if the pad has no net or a net of an ignored class (a keepout would block its own net too). |
| Length maximum checked after routing | Nets longer than a `length (max …)` rule are listed after the import (the router does not enforce maximum lengths). |
| Clearance margin | 5 µm are added to every clearance for the router (`--clearance-margin-um`): it models vias and track ends with polygons, KiCad measures exact circles. |
| Obstacle zones | `--obstacle-zones=NET_OR_CLASS,...` keeps the matching zones as fixed copper that other nets must not cross (e.g. a current path poured on an outer layer), using the zone fill instead of the zone outline so that pads of other nets inside it stay reachable. |
| Copper-to-edge clearance around inner cutouts | KiCad exports an Edge.Cuts contour inside the outline as a keepout exactly on the edge, and `copper_to_edge_clearance_um` only applies to the outer boundary, so traces ran along (or into) cutouts. The holes of the board outline are added again as keepouts inflated by the clearance. |
| Local clearance of pads without a net → keepouts | The DSN export has no per-pad clearance (e.g. 0.6 mm around a fiducial). The pad shape inflated by its local (or footprint) clearance becomes a keepout on its layers. |
| Zone stitching vias kept | A via that no track touches is a dangling tail to Freerouting (`removeTails` after every pass) and, with "remove existing tracks", plain unlocked wiring: a board with 185 GND stitching vias came back with none. The plugin locks such vias for the run (fixed wiring in the DSN) and unlocks them afterwards. |
| "Keep all existing tracks unchanged" (`--keep-existing`) | Unlocked existing tracks are routable wiring that the optimizer may move or reroute. The option locks them for the run (KiCad exports locked tracks as fixed wiring and its session import keeps them) and unlocks them afterwards. |
| "Remove existing tracks" drops unlocked wires/vias from the DSN | Deleting board items inside the action plugin crashed pcbnew (undo snapshot with freed items); locked (`type fix`) tracks are kept. |

## Results (KiCad 10 DRC after re-routing from scratch)

Unconnected items / routing-related violations; see `integrations/kicad/tests/e2e.sh`.

| Board | Original design | Plain Freerouting flow | fastroute plugin |
|---|---|---|---|
| lora_node (4 layers, 445 SMD pads) | 0 / 0 | — | 1 / 0 (53 s incl. optimizer) |
| multichannel_mixer | 0 / 0 | 160 unrouted in the benchmark | 0 / 0 (3.3 s) |
| complex_hierarchy | 0 / 1 | 83 / 3 | 0 / 0 (0.7 s) |
| interf_u | 0 / 3 | 8 / 346 | 0 / 2 (5.6 s) |
| pic_programmer | 0 / 8 | 0 / 118 | 0 / 0 (0.7 s) |
| ecc83-pp | 0 / 0 | 0 / 6 | 0 / 0 (0.3 s) |
| sonde xilinx | 0 / 38 | 1 / 122 | 0 / 36 (0.4 s) |

Remaining violations exist in the original designs as well (pads close to the
board edge, thermal spokes). The lora board's last unconnected item is a GND
zone-to-zone link.

## Optimizer results (benchmark boards, default parallel mode)

`scripts/ab-enhancements.sh` runs each board with and without the improvements
(`--no-enhancements`). Unrouted connections, Freerouting behaviour → fastroute:
bm04 1 → 0, bm05 11 → 2, CM5 8 → 7, StickHub 2 → 1, complex_hierarchy 10 → 8,
all other boards unchanged (no board got worse). Optimizer scores:

| Board | Freerouting behaviour | fastroute |
|---|---|---|
| DAC2020_bm06 | 617 | 718 |
| DAC2020_bm07 | 657 | 752 |
| DAC2020_bm10 | 641 | 711 |
| DAC2020_bm11 | 768 | 859 |
| interf_u | 585 | 619 |
| sonde xilinx | 877 | 928 |
| bm04 / bm05 / CM5 / StickHub (partly routed before) | optimizer skipped | 955 / 569 / 905 / 822 (bm05 now has 2 instead of 11 unrouted, so the optimizer works on a different board) |
| bm01 (score below 0) | no improvement possible (91 s) | −182 → 107 in 4 passes (69 s) |
| lora_node | 328 (+0.45 %) | 435 (+33 %) |

## Diagnosis (`--report`, `--diagnose`)

`--report=FILE` writes a JSON summary of the result. With `--diagnose` every unrouted connection
is routed once more on its own: on the board as loaded (`congestion`: the routed traces are in
the way; `blocked`: the geometry or the rules are), and on the final board as autorouter passes
1 and 10 would, with the unrouted count before, after and after the tail removal. This found
the contact problems above (connections "routed" without changing the unrouted count).
`scripts/bench.py --suite quick -j 3` checks 16 sensitive boards in about 5 minutes; the full
set (`dac,kicad,pcbench:60` plus exported KiCad boards) is for releases.

Not adopted: an automatic neck width (half the narrowest trace width) for boards without one.
It routed some boards completely but left more unrouted on others (karabas-nano 4 → 15,
pogo-pin 10 → 17), also when limited to insertion failures (Aria 24 → 32); after the contact
fixes it no longer helped on balance.

Not adopted either: a PathFinder-style congestion history (McMurchie & Ebeling 1995). Ripped
regions accumulated history on a 1 mm grid per layer (updated between passes, so it stays
deterministic); the history raised the rip-up cost of items there, and/or the cost of routing
through the region. On the quick suite (16 boards + lora_node, unrouted / total time): without
history 101 / 774 s; rip-up cost ×(1 + h) 105 / 896 s; routing cost ×(1 + 0.3 h) 121 / 1018 s;
both 106 / 1095 s. Some oscillating boards improved (S1G 21 → 18, SunLeaf 28 → 26) but others
got worse (Aria 24 → 28–35, bm05 6 → 8) and every variant was slower. A coarse history also
penalises regions that are crowded but passable; a finer or decaying history, or one applied
only to the connections that fail, might still work.

## Open ideas

- CLI users feeding KiCad DSN files directly get none of the plugin's export
  fixes (rule-area keepouts, zone handling, board constraints). A `--kicad`
  preprocessing mode could apply the DSN-only parts (rule areas cannot be
  recognised without the board, though).
- Footprint-local clearances (e.g. a 0.5 mm-pitch LGA with 0.127 mm) are not in
  the DSN export; the router uses the net-class clearance there.
