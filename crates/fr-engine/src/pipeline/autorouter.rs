//! Java `autoroute/pipeline/{BatchAutorouter, AutorouteBatchLoop, AutoroutePassRunner}`
//! (single-thread pass only; `runMultiThread` / `BatchAutorouterThread` are dead code) and the
//! strict DRC part of `AutorouteConnectionRouter`.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;
use std::time::Instant;

use fr_settings::{ExpansionCostFactor, RouterSettings};

use crate::autoroute::router::{route_connection, ConnectionRouterParams};
use crate::autoroute::AutorouteAttemptResult;
use crate::board::{AutorouteAttemptState, ItemKey, ItemSet, RoutingBoard, StopConnectionOption};
use crate::datastructures::StopToken;
use crate::drc::clearance_violation::clearance_violation_count;
use crate::ids::{ItemId, NetNo};

use super::fanout;
use super::history::{board_hash, BoardHistory, MAX_HISTORY_SIZE};
use super::stats::{format_score_with_unfixable, incomplete_count, StatsCache};
use super::{CheckpointKey, PipelineContext};

/// Java `BOARD_RANK_LIMIT`.
const BOARD_RANK_LIMIT: i32 = MAX_HISTORY_SIZE as i32;
/// Java `MAXIMUM_TRIES_ON_THE_SAME_BOARD`.
const MAXIMUM_TRIES_ON_THE_SAME_BOARD: i32 = 3;
/// Java `TIME_LIMIT_TO_PREVENT_ENDLESS_LOOP`.
const TIME_LIMIT_TO_PREVENT_ENDLESS_LOOP: i32 = 1000;
/// Java `STOP_AT_PASS_MINIMUM`.
const STOP_AT_PASS_MINIMUM: i32 = 8;
/// Java `STOP_AT_PASS_MODULO`.
const STOP_AT_PASS_MODULO: i32 = 4;
/// Java `STAGNATION_PASS_LIMIT`.
const STAGNATION_PASS_LIMIT: i32 = 10;
/// Java `FANOUT_RECOVERY_STAGNATION_PASSES`.
const FANOUT_RECOVERY_STAGNATION_PASSES: i32 = 3;
/// Java `STAGNATION_SCORE_THRESHOLD`.
const STAGNATION_SCORE_THRESHOLD: f32 = 0.5;
/// fastroute: progress line interval inside a long autorouting pass.
const PROGRESS_LOG_INTERVAL_SECS: f64 = 30.0;
/// fastroute: at most this many "undo a bad pass" restores per run.
const MAX_REGRESSION_ROLLBACKS: i32 = 3;
/// fastroute: boards kept in the history (Java keeps 30 copies of the board).
const ENHANCED_HISTORY_SIZE: usize = 6;
/// fastroute: passes at least this long use the slow-pass stagnation rule.
const SLOW_PASS_SECS: f64 = 20.0;
/// fastroute: the slow-pass rule stops when the last this-many passes reduced the unrouted
/// items by less than 2 % (at least one) compared with all passes before.
const SLOW_STAGNATION_WINDOW: usize = 3;
/// fastroute: how often a conflicting parallel result is retried in a later batch.
const PARALLEL_RETRIES: u8 = 2;
/// fastroute: connections in flight per thread in the parallel pass.
const PARALLEL_WINDOW_PER_THREAD: usize = 2;

/// Java `BatchAutorouter` (with the state of its `AutoroutePassRunner`).
pub struct BatchAutorouter {
    pub remove_unconnected_vias: bool,
    pub trace_costs: Vec<ExpansionCostFactor>,
    pub retain_autoroute_database: bool,
    pub start_ripup_costs: i32,
    pub trace_pull_tight_accuracy: i32,
    pub total_items_routed: i32,
    pub is_optimizer_autorouter: bool,
    /// fastroute: when set, a pass only routes items of these nets (the optimizer re-routes
    /// the nets of the ripped item instead of every unrouted connection on the board).
    pub net_filter: Option<BTreeSet<NetNo>>,
    /// fastroute improvements (see `PipelineContext::enhancements`).
    pub enhancements: bool,
    /// fastroute: set after a bad pass was undone; failing connections then no longer rip
    /// their whole net (on large nets that is what makes a pass lose hundreds of connections,
    /// and the failure counts survive the undo, so the next pass would do it again).
    pub suppress_net_rip: bool,
    /// fastroute (optimizer): item ids not to route (connections that were already unrouted
    /// when the optimizer started; re-trying them for every candidate only costs time).
    pub skip_items: Option<HashSet<i32>>,
    /// fastroute: threads of the parallel autorouting pass (see `parallel_pass`); 1 = the
    /// sequential Freerouting pass, 0 = decided by `run_batch_loop` (`autorouter.max_threads`
    /// with enhancements, else 1).
    pub pass_threads: usize,
    pass_pool: Option<std::sync::Arc<rayon::ThreadPool>>,
    /// fastroute multi-start: shuffles the order of the first pass's signal items.
    pub order_seed: Option<i64>,
    pub fanout_timed_out: bool,
    /// Java `initialUnroutedCount`.
    pub initial_unrouted_count: i32,
    /// fastroute: the board as it was before routing (pins, keepouts, fixed wiring), to tell
    /// connections that fail because of the routed traces from those that cannot be routed at
    /// all (see [`Self::note_failure`]).
    pristine: Option<std::sync::Arc<RoutingBoard>>,
    /// fastroute: ids of items that cannot be routed even on the pristine board; skipped by
    /// later passes (each attempt is a search over the whole reachable board).
    blocked: HashSet<i32>,
    blocked_checked: HashSet<i32>,
    /// fastroute: a parallel pass stops early once its board has more unrouted connections
    /// than this (the rollback threshold of the batch loop; the pass would be undone anyway).
    pass_abort_above: Option<i32>,
    /// fastroute: the live viewer (`PipelineContext::observer`) of the job's own autorouter.
    observer: Option<super::Observer>,
    // AutoroutePassRunner state
    previous_incomplete_nets: BTreeSet<NetNo>,
    previous_incomplete_count: i32,
    stagnation_count: i32,
}

/// Counters of one pass (for logging).
#[derive(Clone, Copy, Debug, Default)]
pub struct PassCounters {
    pub routed: i32,
    pub not_routed: i32,
    pub skipped: i32,
    pub ripped: i32,
}

impl BatchAutorouter {
    /// Java `new BatchAutorouter(job)`.
    pub fn for_job(board: &RoutingBoard, settings: &RouterSettings) -> Self {
        Self::new(
            board,
            settings,
            !settings.is_fanout_enabled(),
            true,
            settings.get_start_ripup_costs(),
            settings.trace_pull_tight_accuracy.unwrap_or(500),
        )
    }

    /// fastroute diagnostics: routes the connection of `item` once on a copy of `board`, as
    /// pass `pass_no` of the autorouter would (rip-up allowed, rip-up costs of that pass).
    /// Returns the attempt result and the number of items the attempt ripped.
    pub fn diagnose_connection(
        board: &RoutingBoard,
        settings: &RouterSettings,
        item: ItemKey,
        pass_no: i32,
        enhancements: bool,
    ) -> (AutorouteAttemptResult, usize, [i32; 3]) {
        let mut router = Self::for_job(board, settings);
        router.enhancements = enhancements;
        let mut b = board.clone();
        let it = b.item(item);
        if it.net_count() != 1 {
            return (AutorouteAttemptResult::with_details(AutorouteAttemptState::Skipped, "item of several nets"), 0, [0; 3]);
        }
        let net = it.net_number(0);
        let before = incomplete_count(&b, None);
        let mut ripped = ItemSet::new();
        let mut costs: HashMap<ItemKey, i32> = HashMap::new();
        let outcome = route_connection(&mut b, settings, &router.params(), item, net, &mut ripped, Some(&mut costs), pass_no, None);
        let after = incomplete_count(&b, None);
        // as at the end of a pass
        let option = if router.remove_unconnected_vias { StopConnectionOption::None } else { StopConnectionOption::FanoutVia };
        router.remove_tails(&mut b, option, None);
        let after_tails = incomplete_count(&b, None);
        (outcome.result, ripped.len(), [before, after, after_tails])
    }

    /// Tells the live viewer (if any) that a connection of pass `pass_no` was committed.
    fn observe_connection(&self, board: &RoutingBoard, pass_no: i32, done: usize, total: usize, counters: PassCounters) {
        if let Some(cb) = &self.observer {
            cb(board, &super::LiveEvent::Connection { pass_no, done, total, counters });
        }
    }

    /// Java `new BatchAutorouter(thread, board, settings, removeUnconnectedVias,
    /// withPreferredDirections, startRipupCosts, pullTightAccuracy)`.
    pub fn new(
        board: &RoutingBoard,
        settings: &RouterSettings,
        remove_unconnected_vias: bool,
        with_preferred_directions: bool,
        start_ripup_costs: i32,
        pull_tight_accuracy: i32,
    ) -> Self {
        let trace_costs = if with_preferred_directions {
            settings.get_trace_costs()
        } else {
            (0..board.layer_count() as usize)
                .map(|i| {
                    let c = settings.get_preferred_direction_trace_costs(i);
                    ExpansionCostFactor { horizontal: c, vertical: c }
                })
                .collect()
        };
        BatchAutorouter {
            remove_unconnected_vias,
            trace_costs,
            retain_autoroute_database: false,
            start_ripup_costs,
            trace_pull_tight_accuracy: pull_tight_accuracy,
            total_items_routed: 0,
            is_optimizer_autorouter: false,
            net_filter: None,
            enhancements: false,
            suppress_net_rip: false,
            skip_items: None,
            pass_threads: 0,
            pass_pool: None,
            order_seed: None,
            fanout_timed_out: false,
            initial_unrouted_count: 0,
            pristine: None,
            blocked: HashSet::new(),
            blocked_checked: HashSet::new(),
            pass_abort_above: None,
            observer: None,
            previous_incomplete_nets: BTreeSet::new(),
            previous_incomplete_count: -1,
            stagnation_count: 0,
        }
    }

    /// Java `AutoroutePassRunner.resetAntiOscillationState()`.
    pub fn reset_anti_oscillation_state(&mut self) {
        self.previous_incomplete_nets.clear();
        self.previous_incomplete_count = -1;
        self.stagnation_count = 0;
    }

    fn params(&self) -> ConnectionRouterParams {
        ConnectionRouterParams {
            trace_costs: self.trace_costs.clone(),
            start_ripup_costs: self.start_ripup_costs,
            remove_unconnected_vias: self.remove_unconnected_vias,
            retain_autoroute_database: self.retain_autoroute_database,
            trace_pull_tight_accuracy: self.trace_pull_tight_accuracy,
            connect_to_planes: self.enhancements,
        }
    }

    /// Java `removeTails(stopConnectionOption)`.
    pub fn remove_tails(&self, board: &mut RoutingBoard, option: StopConnectionOption, stop: Option<&StopToken>) {
        board.start_marking_changed_area();
        board.remove_trace_tails(-1, option);
        board.opt_changed_area(&[], None, self.trace_pull_tight_accuracy, Some(&self.trace_costs), stop, TIME_LIMIT_TO_PREVENT_ENDLESS_LOOP);
    }

    /// Java `getAutorouteItems(board)`: the connectable, not routable items (pins, conduction
    /// areas, fixed traces and vias) not yet connected to all items of one of their nets, plane
    /// net items first.
    pub fn get_autoroute_items(board: &RoutingBoard) -> Vec<ItemKey> {
        let mut plane_items = Vec::new();
        let mut signal_items = Vec::new();
        let mut handled: HashSet<i32> = HashSet::new();
        for key in board.get_items() {
            let item = board.item(key);
            if !item.is_connectable_class() || item.is_routable() || board.is_stitching_via(key) {
                continue;
            }
            if handled.contains(&item.id().0) {
                continue;
            }
            let mut needs_routing = false;
            let mut has_plane_net = false;
            let net_count = item.net_count();
            for i in 0..net_count {
                let net_no = board.item(key).net_number(i);
                let connected = board.connected_set(key, net_no, false);
                for c in connected.iter() {
                    if board.item(c).net_count() <= 1 {
                        handled.insert(board.item(c).id().0);
                    }
                }
                let net_item_count = board.connectable_item_count(net_no);
                if (connected.len() as i32) < net_item_count && !has_ignored_nets(board, key) {
                    let is_plane = board.rules.nets.get(net_no).map(|n| n.contains_plane()).unwrap_or(false);
                    if is_plane {
                        if connected.iter().any(|c| board.item(c).is_conduction_area()) {
                            continue;
                        }
                        has_plane_net = true;
                    }
                    needs_routing = true;
                }
            }
            if needs_routing {
                if has_plane_net {
                    plane_items.push(key);
                } else {
                    signal_items.push(key);
                }
            }
        }
        plane_items.extend(signal_items);
        plane_items
    }

    /// Java `isPlaneItem(item, board)`.
    fn is_plane_item(board: &RoutingBoard, key: ItemKey) -> bool {
        board.item(key).net_numbers().iter().any(|&n| board.rules.nets.get(n).map(|n| n.contains_plane()).unwrap_or(false))
    }

    /// Rank of the nets `names` in `priority` (net names, most critical first): the lowest rank
    /// among them, or `usize::MAX` when none is listed.
    fn rank_of_nets<'a>(names: impl Iterator<Item = &'a str>, priority: &[String]) -> usize {
        names
            .map(|name| priority.iter().position(|p| p == name).unwrap_or(usize::MAX))
            .min()
            .unwrap_or(usize::MAX)
    }

    /// Rank of `key` in `priority`: the lowest rank of its nets.
    fn priority_rank(board: &RoutingBoard, key: ItemKey, priority: &[String]) -> usize {
        let nets = &board.rules.nets;
        let names = board
            .item(key)
            .net_numbers()
            .iter()
            .filter_map(|&n| nets.get(n))
            .map(|net| net.name.as_str());
        Self::rank_of_nets(names, priority)
    }

    /// Routes the nets listed in `priority` before the others, keeping the rest of the order.
    fn apply_net_priority(board: &RoutingBoard, list: &mut Vec<ItemKey>, priority: &[String]) {
        if !priority.is_empty() {
            list.sort_by_key(|&k| Self::priority_rank(board, k, priority));
        }
    }

    /// Java `AutoroutePassRunner.reorderSingleThreadItems(list, passNo)`.
    fn reorder_items(&self, board: &RoutingBoard, list: &mut Vec<ItemKey>, pass_no: i32, priority: &[String]) {
        let (mut plane_items, mut signal_items): (Vec<ItemKey>, Vec<ItemKey>) = (Vec::new(), Vec::new());
        for &k in list.iter() {
            if Self::is_plane_item(board, k) {
                plane_items.push(k);
            } else {
                signal_items.push(k);
            }
        }
        if signal_items.is_empty() {
            return;
        }
        let rank = |k: &ItemKey| Self::priority_rank(board, *k, priority);
        let sort_key = |k: &ItemKey| {
            let it = board.item(*k);
            (rank(k), if it.net_count() > 0 { it.net_number(0) } else { 0 }, it.id().0)
        };
        if self.stagnation_count >= 2 {
            signal_items.sort_by_key(sort_key);
            let seed = (pass_no as i64).wrapping_mul(1000003).wrapping_add(board.rules.nets.max_net_number() as i64);
            let mut rnd = fr_jcompat::random::JavaRandom::new(seed);
            fr_jcompat::random::shuffle(&mut signal_items, &mut rnd);
            signal_items.sort_by_key(rank);
            log::debug!(
                "Pass #{pass_no}: applied deterministic permutation to {} signal items (stagnation count: {}).",
                signal_items.len(),
                self.stagnation_count
            );
        } else if !self.previous_incomplete_nets.is_empty() {
            let (mut persistent, mut other): (Vec<ItemKey>, Vec<ItemKey>) = (Vec::new(), Vec::new());
            for k in signal_items {
                if board.item(k).net_numbers().iter().any(|n| self.previous_incomplete_nets.contains(n)) {
                    persistent.push(k);
                } else {
                    other.push(k);
                }
            }
            persistent.sort_by_key(sort_key);
            other.sort_by_key(sort_key);
            persistent.extend(other);
            signal_items = persistent;
        } else {
            signal_items.sort_by_key(sort_key);
        }
        list.clear();
        list.extend(plane_items);
        list.extend(signal_items);
    }

    /// Java `autoroutePass(passNo)` (`AutoroutePassRunner.runSingleThread`): routes all
    /// incomplete items once. Returns false if the board is already completely routed (or
    /// nothing was attempted).
    pub fn autoroute_pass(&mut self, board: &mut RoutingBoard, settings: &RouterSettings, pass_no: i32, stop: &StopToken) -> (bool, PassCounters) {
        let r = catch_unwind(AssertUnwindSafe(|| self.autoroute_pass_impl(board, settings, pass_no, stop)));
        match r {
            Ok(v) => v,
            Err(_) => {
                log::error!("Something went wrong during the auto-routing");
                (false, PassCounters::default())
            }
        }
    }

    fn autoroute_pass_impl(&mut self, board: &mut RoutingBoard, settings: &RouterSettings, pass_no: i32, stop: &StopToken) -> (bool, PassCounters) {
        if self.pass_threads > 1 && !self.is_optimizer_autorouter && self.net_filter.is_none() && !settings.is_strict_drc() {
            return self.autoroute_pass_parallel(board, settings, pass_no, stop);
        }
        let mut items = Self::get_autoroute_items(board);
        if let Some(filter) = &self.net_filter {
            items.retain(|&k| board.item(k).net_numbers().iter().any(|n| filter.contains(n)));
        }
        if let Some(skip) = &self.skip_items {
            items.retain(|&k| !skip.contains(&board.item(k).id().0));
        }
        if !self.blocked.is_empty() {
            items.retain(|&k| !self.blocked.contains(&board.item(k).id().0));
        }
        if items.is_empty() {
            return (false, PassCounters::default());
        }
        let priority = settings.get_net_priority();
        if pass_no > 1 {
            self.reorder_items(board, &mut items, pass_no, &priority);
        } else if let Some(seed) = self.order_seed {
            let (plane, mut signal): (Vec<ItemKey>, Vec<ItemKey>) = items.iter().partition(|&&k| Self::is_plane_item(board, k));
            let mut rnd = fr_jcompat::random::JavaRandom::new(seed);
            fr_jcompat::random::shuffle(&mut signal, &mut rnd);
            items = plane;
            items.extend(signal);
        }
        // Last, so that the shuffle of a first pass cannot undo the priority bands.
        Self::apply_net_priority(board, &mut items, &priority);
        let params = self.params();
        let ids: Vec<ItemId> = items.iter().map(|&k| board.item(k).id()).collect();
        let mut c = PassCounters::default();
        let max_items = settings.autorouter.max_items;
        let mut item_index = 0;
        let pass_start = Instant::now();
        let mut next_progress = PROGRESS_LOG_INTERVAL_SECS;
        while item_index < items.len() {
            let mut current = items[item_index];
            item_index += 1;
            if stop.is_stop_autorouter_requested() {
                break;
            }
            if !self.is_optimizer_autorouter && pass_start.elapsed().as_secs_f64() >= next_progress {
                next_progress += PROGRESS_LOG_INTERVAL_SECS;
                log::info!(
                    "Auto-routing pass #{pass_no}: {} of {} items after {:.0} s (routed {}, failed {}, ripped {}).",
                    item_index - 1,
                    items.len(),
                    pass_start.elapsed().as_secs_f64(),
                    c.routed,
                    c.not_routed,
                    c.ripped
                );
            }
            let mut i = 0;
            while i < board.item(current).net_count() {
                if stop.is_stop_autorouter_requested() {
                    break;
                }
                if let Some(m) = max_items {
                    if m > 0 && self.total_items_routed >= m {
                        log::info!("Max items limit reached ({m}). Stopping auto-router.");
                        stop.request_stop();
                        break;
                    }
                }
                self.total_items_routed += 1;
                board.start_marking_changed_area();
                let net_no = board.item(current).net_number(i);
                let mut ripped = ItemSet::new();
                let mut ripped_costs: HashMap<ItemKey, i32> = HashMap::new();
                let snapshot = if settings.is_strict_drc() { Some(board.deep_copy()) } else { None };
                let outcome = route_connection(board, settings, &params, current, net_no, &mut ripped, Some(&mut ripped_costs), pass_no, Some(stop));
                let mut result = outcome.result;
                if result.state == AutorouteAttemptState::Routed {
                    if let Some(max_id) = outcome.max_item_id_before_route {
                        // AutorouteConnectionRouter.applyStrictDrcAfterRoute
                        if settings.is_strict_drc() || pass_no >= 3 {
                            if let Some(rejection) = enforce_strict_drc(board, net_no, max_id) {
                                if let Some(s) = snapshot {
                                    // Java replaces the board by the snapshot and continues with
                                    // the item objects of the old board; the port maps the
                                    // remaining items of the pass list to the snapshot by id.
                                    *board = s;
                                    for j in (item_index - 1)..items.len() {
                                        if let Some(k) = board.get_item(ids[j]) {
                                            items[j] = k;
                                        }
                                    }
                                    current = items[item_index - 1];
                                }
                                result = rejection;
                            }
                        }
                    }
                }
                match result.state {
                    AutorouteAttemptState::Routed => c.routed += 1,
                    AutorouteAttemptState::AlreadyConnected | AutorouteAttemptState::NoUnconnectedNets | AutorouteAttemptState::ConnectedToPlane => {
                        c.skipped += 1
                    }
                    _ => {
                        record_failure(board, current, pass_no, &result);
                        let failure_count = board.failure_log.get_failure_count(&board.basic, current);
                        self.note_failure(board, current, failure_count, settings);
                        // Java rips the whole net after every failure from the second one on; a
                        // connection that can never be routed then tears down its (possibly
                        // large) net every pass. fastroute rips it once.
                        let rip_net = if self.enhancements { failure_count == 2 && !self.suppress_net_rip } else { failure_count >= 2 };
                        if rip_net {
                            let net_no = board.item(current).net_number(i);
                            let to_rip: Vec<ItemKey> = board
                                .get_connectable_items(net_no)
                                .into_iter()
                                .filter(|&k| {
                                    let it = board.item(k);
                                    (it.is_trace() || it.is_via()) && !it.is_user_fixed() && it.net_count() == 1
                                })
                                .collect();
                            if !to_rip.is_empty() {
                                board.remove_items(to_rip);
                            }
                        }
                        c.not_routed += 1;
                    }
                }
                c.ripped += ripped.len() as i32;
                i += 1;
            }
            self.observe_connection(board, pass_no, item_index, items.len(), c);
        }
        if self.remove_unconnected_vias {
            self.remove_tails(board, StopConnectionOption::None, Some(stop));
        } else {
            self.remove_tails(board, StopConnectionOption::FanoutVia, Some(stop));
        }
        let mut current_incomplete_nets = BTreeSet::new();
        let incomplete = incomplete_count(board, Some(&mut current_incomplete_nets));
        if self.previous_incomplete_count >= 0 && incomplete >= self.previous_incomplete_count {
            self.stagnation_count += 1;
        } else {
            self.stagnation_count = 0;
        }
        self.previous_incomplete_count = incomplete;
        self.previous_incomplete_nets = current_incomplete_nets;
        (c.routed > 0 || c.not_routed > 0, c)
    }

    /// fastroute: the autorouting pass on several threads (see `super::parallel_pass`).
    ///
    /// A rolling window: up to `window` connections are in flight, each routed on a clone of
    /// the board as it was when the connection was dispatched; results are committed in
    /// dispatch order, and every commit dispatches the next connection. A slow connection
    /// therefore only holds back the commits after it, not the other threads. Dispatch
    /// decisions only depend on the commits, so the result does not depend on thread timing.
    fn autoroute_pass_parallel(&mut self, board: &mut RoutingBoard, settings: &RouterSettings, pass_no: i32, stop: &StopToken) -> (bool, PassCounters) {
        use super::parallel_pass as pp;
        use std::collections::VecDeque;
        use std::sync::mpsc;
        let mut items = Self::get_autoroute_items(board);
        if !self.blocked.is_empty() {
            items.retain(|&k| !self.blocked.contains(&board.item(k).id().0));
        }
        if items.is_empty() {
            return (false, PassCounters::default());
        }
        let priority = settings.get_net_priority();
        if pass_no > 1 {
            self.reorder_items(board, &mut items, pass_no, &priority);
        } else if let Some(seed) = self.order_seed {
            let (plane, mut signal): (Vec<ItemKey>, Vec<ItemKey>) = items.iter().partition(|&&k| Self::is_plane_item(board, k));
            let mut rnd = fr_jcompat::random::JavaRandom::new(seed);
            fr_jcompat::random::shuffle(&mut signal, &mut rnd);
            items = plane;
            items.extend(signal);
        }
        // Last, so that the shuffle of a first pass cannot undo the priority bands.
        Self::apply_net_priority(board, &mut items, &priority);
        let params = self.params();
        let threads = self.pass_threads;
        let window = threads * PARALLEL_WINDOW_PER_THREAD;
        // one extra thread: the committing loop runs inside the pool as well
        let pool = self
            .pass_pool
            .get_or_insert_with(|| {
                std::sync::Arc::new(rayon::ThreadPoolBuilder::new().num_threads(threads + 1).build().expect("autorouter thread pool"))
            })
            .clone();
        let margin = (pp::FOOTPRINT_MARGIN_MM * pp::units_per_mm(board)) as i64;
        let mut queue: VecDeque<ItemId> = items.iter().map(|&k| board.item(k).id()).collect();
        let total = queue.len();
        let mut c = PassCounters::default();
        let max_items = settings.autorouter.max_items;
        let pass_start = Instant::now();
        let mut next_progress = PROGRESS_LOG_INTERVAL_SECS;
        let (mut copied, mut retried, mut rerouted) = (0usize, 0usize, 0usize);
        let (mut reject_changed, mut reject_clearance) = (0usize, 0usize);
        let mut retries: HashMap<i32, u8> = HashMap::new();
        let mut processed = 0usize;
        let params_ref = &params;

        /// A finished connection: (ticket, result, ripped count, the worker's condensed changes).
        type Done = (usize, AutorouteAttemptResult, i32, Option<pp::Condensed>);

        pool.install(|| {
            rayon::scope(|scope| {
                let (tx, rx) = mpsc::channel::<Done>();
                // in flight: ticket -> (item, net)
                let mut in_flight: std::collections::BTreeMap<usize, (ItemId, NetNo)> = std::collections::BTreeMap::new();
                let mut arrived: HashMap<usize, Done> = HashMap::new();
                let mut flight_rects: HashMap<usize, pp::Rect> = HashMap::new();
                let mut footprints: HashMap<i32, Option<pp::Rect>> = HashMap::new();
                let mut next_ticket = 0usize;
                let mut next_commit = 0usize;
                // The board as it is now, shared by all connections dispatched before the next
                // change of the board (cleared whenever the board may have changed).
                let mut snapshot: Option<Arc<RoutingBoard>> = None;
                'pass: loop {
                    if stop.is_stop_autorouter_requested() {
                        break;
                    }
                    // Dispatch until the window is full (never two connections of one net at once).
                    while in_flight.len() < window && !queue.is_empty() {
                        let busy: BTreeSet<NetNo> = in_flight.values().map(|&(_, n)| n).collect();
                        let eligible = |id: ItemId| match board.get_item(id) {
                            None => true,
                            Some(k) => {
                                let it = board.item(k);
                                it.net_count() != 1 || !busy.contains(&it.net_number(0))
                            }
                        };
                        // prefer a connection away from the ones in flight (fewer conflicts)
                        let mut pos = None;
                        for (i, &id) in queue.iter().take(window * 8).enumerate() {
                            if !eligible(id) {
                                continue;
                            }
                            let fp = match board.get_item(id) {
                                Some(k) if board.item(k).net_count() == 1 => *footprints
                                    .entry(id.0)
                                    .or_insert_with(|| pp::connection_footprint(board, k, board.item(k).net_number(0), margin)),
                                _ => None,
                            };
                            if fp.map_or(true, |r| !flight_rects.values().any(|q| pp::overlaps(q, &r))) {
                                pos = Some(i);
                                break;
                            }
                        }
                        let pos = pos.or_else(|| queue.iter().take(window * 8).position(|&id| eligible(id)));
                        let Some(pos) = pos else { break };
                        let id = queue.remove(pos).unwrap();
                        let Some(key) = board.get_item(id) else {
                            processed += 1;
                            continue;
                        };
                        if board.item(key).net_count() != 1 {
                            // rare (items of several nets): route it alone, on the board itself
                            if !in_flight.is_empty() {
                                queue.push_front(id);
                                break;
                            }
                            let nets_of: Vec<NetNo> = board.item(key).net_numbers().to_vec();
                            snapshot = None;
                            for net in nets_of {
                                self.route_one(board, settings, params_ref, id, net, pass_no, stop, &mut c);
                            }
                            processed += 1;
                            continue;
                        }
                        let net = board.item(key).net_number(0);
                        let ticket = next_ticket;
                        next_ticket += 1;
                        in_flight.insert(ticket, (id, net));
                        if let Some(Some(r)) = footprints.get(&id.0) {
                            flight_rects.insert(ticket, *r);
                        }
                        let base = snapshot.get_or_insert_with(|| Arc::new(board.clone())).clone();
                        let tx = tx.clone();
                        scope.spawn(move |_| {
                            let mut w = (*base).clone();
                            let done = match w.get_item(id) {
                                None => (ticket, AutorouteAttemptResult::new(AutorouteAttemptState::Failed), 0, None),
                                Some(key) => {
                                    w.start_marking_changed_area();
                                    let mut ripped = ItemSet::new();
                                    let mut costs: HashMap<ItemKey, i32> = HashMap::new();
                                    let outcome =
                                        route_connection(&mut w, settings, params_ref, key, net, &mut ripped, Some(&mut costs), pass_no, Some(stop));
                                    let mut result = outcome.result;
                                    let mut keep = result.state == AutorouteAttemptState::Routed;
                                    if keep && pass_no >= 3 {
                                        if let Some(max_id) = outcome.max_item_id_before_route {
                                            if let Some(rejection) = enforce_strict_drc(&mut w, net, max_id) {
                                                result = rejection;
                                                keep = false;
                                            }
                                        }
                                    }
                                    // only the changes are kept: the worker board and (unless
                                    // shared) the batch-start board are dropped here
                                    let changes = keep.then(|| {
                                        let base_index = pp::index(&base);
                                        let ch = pp::changes(&base, &base_index, &w, margin, net);
                                        pp::condense(ch, &base, &w)
                                    });
                                    (ticket, result, ripped.len() as i32, changes)
                                }
                            };
                            let _ = tx.send(done);
                        });
                    }
                    if in_flight.is_empty() {
                        if queue.is_empty() {
                            break;
                        }
                        continue;
                    }
                    // Wait for the oldest ticket, then commit it.
                    while !arrived.contains_key(&next_commit) {
                        match rx.recv() {
                            Ok(d) => {
                                arrived.insert(d.0, d);
                            }
                            Err(_) => break 'pass,
                        }
                    }
                    let (_, result, ripped_count, work) = arrived.remove(&next_commit).unwrap();
                    // whatever follows may change the board
                    snapshot = None;
                    let (id, net) = in_flight.remove(&next_commit).unwrap();
                    flight_rects.remove(&next_commit);
                    next_commit += 1;
                    if let Some(m) = max_items {
                        if m > 0 && self.total_items_routed >= m {
                            log::info!("Max items limit reached ({m}). Stopping auto-router.");
                            stop.request_stop();
                            break 'pass;
                        }
                    }
                    let Some(key) = board.get_item(id) else {
                        processed += 1;
                        continue;
                    };
                    match result.state {
                        AutorouteAttemptState::Routed => {
                            let cd = work.expect("routed worker changes");
                            let applied = if !cd.changes.copyable {
                                None
                            } else {
                                match pp::apply(&cd, board) {
                                    Ok(b) => Some(b),
                                    Err(pp::Reject::Changed) => {
                                        reject_changed += 1;
                                        None
                                    }
                                    Err(pp::Reject::Clearance) => {
                                        reject_clearance += 1;
                                        None
                                    }
                                }
                            };
                            if let Some(next) = applied {
                                *board = next;
                                self.total_items_routed += 1;
                                c.routed += 1;
                                c.ripped += ripped_count;
                                copied += 1;
                                processed += 1;
                            } else if *retries.entry(id.0).or_insert(0) < PARALLEL_RETRIES {
                                // routed on a board that changed meanwhile: dispatch it again
                                *retries.get_mut(&id.0).unwrap() += 1;
                                retried += 1;
                                queue.push_front(id);
                            } else {
                                rerouted += 1;
                                self.total_items_routed += 1;
                                self.route_one(board, settings, params_ref, id, net, pass_no, stop, &mut c);
                                processed += 1;
                            }
                        }
                        AutorouteAttemptState::AlreadyConnected | AutorouteAttemptState::NoUnconnectedNets | AutorouteAttemptState::ConnectedToPlane => {
                            self.total_items_routed += 1;
                            c.skipped += 1;
                            processed += 1;
                        }
                        _ => {
                            self.total_items_routed += 1;
                            c.ripped += ripped_count;
                            self.handle_failure(board, settings, key, 0, pass_no, &result, &mut c);
                            processed += 1;
                        }
                    }
                    self.observe_connection(board, pass_no, processed, total, c);
                    if pass_start.elapsed().as_secs_f64() >= next_progress {
                        next_progress += PROGRESS_LOG_INTERVAL_SECS;
                        let now = incomplete_count(board, None);
                        log::info!(
                            "Auto-routing pass #{pass_no}: {processed} of {total} items after {:.0} s, {now} unrouted (routed {}, failed {}, ripped {}; parallel: {copied} copied, {retried} retried, {rerouted} re-routed).",
                            pass_start.elapsed().as_secs_f64(),
                            c.routed,
                            c.not_routed,
                            c.ripped,
                        );
                        if let Some(limit) = self.pass_abort_above {
                            if now > limit {
                                log::info!(
                                    "Auto-routing pass #{pass_no}: {now} unrouted, more than the best board so far allows ({limit}): stopping the pass early (it will be undone)."
                                );
                                break 'pass;
                            }
                        }
                    }
                }
                // (a stop leaves tasks running: they end quickly, their results are dropped)
                drop(tx);
            });
        });
        log::debug!(
            target: "fr_engine::pipeline::diag",
            "parallel pass #{pass_no}: {copied} results copied, {retried} retried, {rerouted} re-routed sequentially (changed items {reject_changed}, clearance {reject_clearance})"
        );
        if self.remove_unconnected_vias {
            self.remove_tails(board, StopConnectionOption::None, Some(stop));
        } else {
            self.remove_tails(board, StopConnectionOption::FanoutVia, Some(stop));
        }
        let mut current_incomplete_nets = BTreeSet::new();
        let incomplete = incomplete_count(board, Some(&mut current_incomplete_nets));
        if self.previous_incomplete_count >= 0 && incomplete >= self.previous_incomplete_count {
            self.stagnation_count += 1;
        } else {
            self.stagnation_count = 0;
        }
        self.previous_incomplete_count = incomplete;
        self.previous_incomplete_nets = current_incomplete_nets;
        (c.routed > 0 || c.not_routed > 0, c)
    }

    /// One connection of the parallel pass, routed on the board itself (as in the sequential
    /// pass, without strict-DRC snapshots, which the parallel pass does not use).
    #[allow(clippy::too_many_arguments)]
    fn route_one(
        &mut self,
        board: &mut RoutingBoard,
        settings: &RouterSettings,
        params: &ConnectionRouterParams,
        id: ItemId,
        net: NetNo,
        pass_no: i32,
        stop: &StopToken,
        c: &mut PassCounters,
    ) {
        let Some(key) = board.get_item(id) else { return };
        board.start_marking_changed_area();
        let mut ripped = ItemSet::new();
        let mut costs: HashMap<ItemKey, i32> = HashMap::new();
        let outcome = route_connection(board, settings, params, key, net, &mut ripped, Some(&mut costs), pass_no, Some(stop));
        let mut result = outcome.result;
        if result.state == AutorouteAttemptState::Routed && pass_no >= 3 {
            if let Some(max_id) = outcome.max_item_id_before_route {
                if let Some(rejection) = enforce_strict_drc(board, net, max_id) {
                    result = rejection;
                }
            }
        }
        c.ripped += ripped.len() as i32;
        match result.state {
            AutorouteAttemptState::Routed => c.routed += 1,
            AutorouteAttemptState::AlreadyConnected | AutorouteAttemptState::NoUnconnectedNets | AutorouteAttemptState::ConnectedToPlane => {
                c.skipped += 1
            }
            _ => {
                if let Some(key) = board.get_item(id) {
                    let net_index = board.item(key).net_numbers().iter().position(|&n| n == net).unwrap_or(0);
                    self.handle_failure(board, settings, key, net_index, pass_no, &result, c);
                }
            }
        }
    }

    /// Failure bookkeeping of a connection (as in the sequential pass): logs it and rips the
    /// item's net once after its second failure.
    /// fastroute: after the second failure of an item, routes it once alone on the pristine
    /// board; if it fails there as well, the item is skipped by the later passes.
    fn note_failure(&mut self, board: &RoutingBoard, key: ItemKey, failure_count: i32, settings: &RouterSettings) {
        if failure_count < 2 {
            return;
        }
        let Some(pristine) = &self.pristine else { return };
        let id = board.item(key).id().0;
        if !self.blocked_checked.insert(id) {
            return;
        }
        let mut b = (**pristine).clone();
        let Some(k) = b.get_item(ItemId(id)) else { return };
        let r = b.autoroute(k, settings, settings.get_via_costs(), None, None);
        if matches!(r.state, AutorouteAttemptState::Failed | AutorouteAttemptState::InsertError) {
            log::debug!(target: "fr_engine::pipeline::diag", "item #{id} cannot be routed on the pristine board ({}): skipped from now on", r.details);
            self.blocked.insert(id);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_failure(&mut self, board: &mut RoutingBoard, settings: &RouterSettings, current: ItemKey, net_index: usize, pass_no: i32, result: &AutorouteAttemptResult, c: &mut PassCounters) {
        record_failure(board, current, pass_no, result);
        let failure_count = board.failure_log.get_failure_count(&board.basic, current);
        self.note_failure(board, current, failure_count, settings);
        let rip_net = failure_count == 2 && !self.suppress_net_rip;
        if rip_net {
            let net_no = board.item(current).net_number(net_index as i32);
            let to_rip: Vec<ItemKey> = board
                .get_connectable_items(net_no)
                .into_iter()
                .filter(|&k| {
                    let it = board.item(k);
                    (it.is_trace() || it.is_via()) && !it.is_user_fixed() && it.net_count() == 1
                })
                .collect();
            if !to_rip.is_empty() {
                board.remove_items(to_rip);
            }
        }
        c.not_routed += 1;
    }

    /// Java `autoroutePassesForOptimizingItem(...)` body after the construction: routes up to
    /// `max_pass_count` passes, then removes the tails. Returns the number of passes.
    pub fn autoroute_passes_for_optimizing_item(&mut self, board: &mut RoutingBoard, settings: &RouterSettings, max_pass_count: i32, stop: &StopToken) -> i32 {
        self.is_optimizer_autorouter = true;
        let mut still_unrouted = true;
        let mut current_pass_no = 1;
        while still_unrouted && !stop.is_stop_autorouter_requested() && current_pass_no <= max_pass_count {
            still_unrouted = self.autoroute_pass(board, settings, current_pass_no, stop).0;
            current_pass_no += 1;
        }
        self.remove_tails(board, StopConnectionOption::None, Some(stop));
        if !still_unrouted {
            current_pass_no -= 1;
        }
        current_pass_no
    }

    /// Java `runBatchLoop()` (`AutorouteBatchLoop.run`): fanout, then autoroute passes until the
    /// board is completed or a stop rule fires. Returns true if not stopped.
    #[allow(clippy::collapsible_if)]
    pub fn run_batch_loop(&mut self, board: &mut RoutingBoard, settings: &RouterSettings, ctx: &PipelineContext) -> bool {
        let stop = &ctx.stop;
        self.enhancements = ctx.enhancements;
        self.observer = if self.is_optimizer_autorouter { None } else { ctx.observer.clone() };
        if self.pass_threads == 0 {
            self.pass_threads = if ctx.enhancements && !self.is_optimizer_autorouter {
                settings.autorouter.max_threads.unwrap_or(1).max(1) as usize
            } else {
                1
            };
        }
        let any_routable = (0..settings.get_layer_count())
            .any(|i| settings.get_layer_active(i) && board.layer_structure.layers.get(i).map(|l| l.is_signal).unwrap_or(false));
        if !any_routable {
            log::error!("Cannot start autorouter: all layers are disabled.");
            return false;
        }
        let optimizer_threads = settings.optimizer.max_threads.map(|t| t.max(1)).unwrap_or(1);
        log::info!(
            "Pipeline thread limits: autorouter.max_threads={}, optimizer.max_threads={}.",
            settings.autorouter.max_threads.unwrap_or(1),
            optimizer_threads
        );
        self.initial_unrouted_count = incomplete_count(board, None);
        self.reset_anti_oscillation_state();
        if self.enhancements && !self.is_optimizer_autorouter && self.pristine.is_none() {
            self.pristine = Some(std::sync::Arc::new(board.clone()));
        }
        // fastroute: the history holds board copies; the restore rules look at the best few
        let mut bh = if ctx.enhancements { BoardHistory::with_max_size(ENHANCED_HISTORY_SIZE) } else { BoardHistory::new() };
        let mut stats = StatsCache::new();

        if settings.is_fanout_enabled() {
            if board.get_smd_pins().is_empty() {
                log::info!("Fanout stage is enabled but skipped because the board has no SMD pins.");
            } else {
                let t = Instant::now();
                ctx.observe(board, &super::LiveEvent::Stage("fanout"));
                let summary = fanout::fanout_board(board, settings, ctx);
                self.fanout_timed_out = summary.timed_out;
                log::info!(
                    "Fanout stage {} {} passes, completed in {:.2} seconds.",
                    if summary.timed_out { "completed with timeout:" } else if stop.is_stop_autorouter_requested() { "interrupted:" } else { "completed:" },
                    summary.completed_passes,
                    t.elapsed().as_secs_f64()
                );
            }
        }

        let current_unrouted = incomplete_count(board, None);
        if ctx.enhancements && !self.is_optimizer_autorouter && ctx.checkpoint.is_some() {
            // fastroute: the board after the fanout is the first checkpoint, so a run stopped
            // during its first pass leaves a session file behind
            let s = stats.score(board, settings);
            ctx.checkpoint(board, CheckpointKey { incomplete: s.incomplete_count, violations: s.clearance_violation_count, stage: 0, score: s.router_score });
        }
        // fastroute: unrouted connections of ignored net classes (constant: never routed)
        let ignored = if ctx.enhancements { super::stats::ignored_incomplete_count(board) } else { 0 };
        let is_router_enabled = settings.get_run_router() && settings.autorouter.max_passes.map(|m| m >= 0).unwrap_or(true);
        let stage_start = Instant::now();
        if is_router_enabled {
            if !self.is_optimizer_autorouter {
                ctx.observe(board, &super::LiveEvent::Stage("autorouter"));
            }
            let s = stats.score(board, settings);
            log::info!(
                "Auto-routing stage started with baseline score {:.2} for {} unrouted item{}{}.",
                s.router_score as f64,
                current_unrouted,
                if current_unrouted == 1 { "" } else { "s" },
                if ignored > 0 { format!(" ({ignored} of them in ignored net classes, not routed)") } else { String::new() }
            );
        }
        let mut continue_autorouting = is_router_enabled;
        let mut current_pass = 1;
        let mut consecutive_no_improvement_passes = 0;
        let mut fanout_recovery_applied = false;
        let mut last_best_score = f32::NEG_INFINITY;
        let mut global_best_score = f32::NEG_INFINITY;
        let mut pass_of_best_score = 0;
        let mut incomplete_count_at_best_score = 0;
        let mut fewest_incomplete = i32::MAX;
        let mut incomplete_history: Vec<i32> = Vec::new();
        let mut rollbacks = 0;
        // fastroute: router.autorouter.min_passes keeps the stagnation rules quiet this long
        let min_passes = settings.autorouter.min_passes.unwrap_or(0);
        while continue_autorouting && !stop.is_stop_autorouter_requested() {
            if let Some(m) = settings.autorouter.max_passes {
                if m > 0 && current_pass > m {
                    stop.request_stop_autorouter();
                    break;
                }
            }
            let pass_start = Instant::now();
            {
                let hash = board_hash(board);
                let b: &RoutingBoard = board;
                bh.add_with(b, hash, || stats.score(b, settings).router_score);
            }
            self.pass_abort_above = (ctx.enhancements && !self.is_optimizer_autorouter && current_pass >= 2 && fewest_incomplete != i32::MAX)
                .then(|| fewest_incomplete + ((fewest_incomplete - ignored) * 3 / 10).max(20));
            let (cont, counters) = self.autoroute_pass(board, settings, current_pass, stop);
            continue_autorouting = cont;
            let mut after = stats.score(board, settings);

            if bh.size() >= STOP_AT_PASS_MINIMUM as usize || stop.is_stop_autorouter_requested() {
                if (current_pass % STOP_AT_PASS_MODULO == 0 && current_pass >= STOP_AT_PASS_MINIMUM) || stop.is_stop_autorouter_requested() {
                    if bh.get_max_score() > after.router_score {
                        let Some(to_restore) = bh.restore_board(MAXIMUM_TRIES_ON_THE_SAME_BOARD) else {
                            log::info!("The router was not able to improve the board, stopping the auto-router.");
                            stop.request_stop_autorouter();
                            break;
                        };
                        let rank = bh.get_rank(&to_restore);
                        if rank > BOARD_RANK_LIMIT {
                            stop.request_stop_autorouter();
                            break;
                        }
                        *board = to_restore;
                        self.reset_anti_oscillation_state();
                        consecutive_no_improvement_passes = 0;
                        after = stats.score(board, settings);
                        last_best_score = after.router_score;
                        log::debug!(
                            "Restoring an earlier board that has the score of {}.",
                            format_score_with_unfixable(after.router_score, after.incomplete_count, after.clearance_violation_count, after.unfixable_violation_count)
                        );
                    }
                }
            }
            if !self.is_optimizer_autorouter {
                log::info!(
                    "Auto-routing pass #{} was completed in {:.2} seconds with score {} (routed {}, failed {}, ripped {}).",
                    current_pass,
                    pass_start.elapsed().as_secs_f64(),
                    format_score_with_unfixable(after.router_score, after.incomplete_count, after.clearance_violation_count, after.unfixable_violation_count),
                    counters.routed,
                    counters.not_routed,
                    counters.ripped
                );
            }

            if !self.is_optimizer_autorouter {
                ctx.observe(board, &super::LiveEvent::RouterPass {
                    pass_no: current_pass,
                    secs: pass_start.elapsed().as_secs_f64(),
                    counters,
                    incomplete: after.incomplete_count,
                    violations: after.clearance_violation_count,
                    score: after.router_score,
                });
                ctx.checkpoint(board, CheckpointKey {
                    incomplete: after.incomplete_count,
                    violations: after.clearance_violation_count,
                    stage: 0,
                    score: after.router_score,
                });
            }
            if ctx.enhancements && !self.is_optimizer_autorouter && continue_autorouting {
                // fastroute: a pass that loses much more than it gains (ripped nets) is undone:
                // the next pass starts from the best board so far. Java keeps routing from the
                // worse board and restores the history only every few passes.
                let best = fewest_incomplete.min(after.incomplete_count);
                if current_pass >= 2
                    && rollbacks < MAX_REGRESSION_ROLLBACKS
                    && after.incomplete_count > fewest_incomplete + ((fewest_incomplete - ignored) * 3 / 10).max(20)
                {
                    if let Some(b) = bh.restore_best_board() {
                        let lost = after.incomplete_count;
                        *board = b;
                        self.reset_anti_oscillation_state();
                        after = stats.score(board, settings);
                        rollbacks += 1;
                        self.suppress_net_rip = true;
                        log::info!(
                            "Auto-routing pass #{current_pass} left {lost} items unrouted (best so far {fewest_incomplete}): continuing from the best board ({} unrouted); failing connections no longer rip their whole net.",
                            after.incomplete_count
                        );
                    }
                }
                fewest_incomplete = best.min(after.incomplete_count);
                incomplete_history.push(after.incomplete_count);
                // fastroute: on boards where a pass takes long, stop once the passes no longer
                // pay off (Java's stagnation rules only start after 8 passes).
                let n = incomplete_history.len();
                if pass_start.elapsed().as_secs_f64() >= SLOW_PASS_SECS && n > SLOW_STAGNATION_WINDOW && current_pass >= min_passes {
                    let before = *incomplete_history[..n - SLOW_STAGNATION_WINDOW].iter().min().unwrap();
                    let recent = *incomplete_history[n - SLOW_STAGNATION_WINDOW..].iter().min().unwrap();
                    if recent - ignored > 0 && before - recent < ((before - ignored) / 50).max(1) {
                        log::info!(
                            "Stopping the auto-router: the last {SLOW_STAGNATION_WINDOW} passes (about {:.0} s each) reduced the unrouted items only from {before} to {recent}.",
                            pass_start.elapsed().as_secs_f64()
                        );
                        stop.request_stop_autorouter();
                        break;
                    }
                }
            }

            if current_pass >= STOP_AT_PASS_MINIMUM && current_pass >= min_passes && continue_autorouting {
                if after.router_score > last_best_score + STAGNATION_SCORE_THRESHOLD {
                    consecutive_no_improvement_passes = 0;
                    last_best_score = after.router_score;
                } else {
                    consecutive_no_improvement_passes += 1;
                    if settings.is_fanout_enabled()
                        && !fanout_recovery_applied
                        && after.incomplete_count > 0
                        && consecutive_no_improvement_passes >= FANOUT_RECOVERY_STAGNATION_PASSES
                    {
                        let before = after.incomplete_count;
                        self.remove_tails(board, StopConnectionOption::None, Some(stop));
                        after = stats.score(board, settings);
                        last_best_score = after.router_score;
                        consecutive_no_improvement_passes = 0;
                        fanout_recovery_applied = true;
                        log::debug!(
                            "Applied one-time fanout recovery cleanup (removed fanout tails/vias). Incompletes: {} -> {}.",
                            before,
                            after.incomplete_count
                        );
                    }
                    if consecutive_no_improvement_passes >= STAGNATION_PASS_LIMIT {
                        log::info!(
                            "The router's score ({:.2}) has not improved by more than {} points in the last {} passes ({} item{} still unconnected). Stopping the auto-router.",
                            after.router_score as f64,
                            STAGNATION_SCORE_THRESHOLD,
                            STAGNATION_PASS_LIMIT,
                            after.incomplete_count,
                            if after.incomplete_count == 1 { "" } else { "s" }
                        );
                        stop.request_stop_autorouter();
                        break;
                    }
                }
                if after.router_score > global_best_score + STAGNATION_SCORE_THRESHOLD {
                    global_best_score = after.router_score;
                    pass_of_best_score = current_pass;
                    incomplete_count_at_best_score = after.incomplete_count;
                } else if current_pass - pass_of_best_score >= STAGNATION_PASS_LIMIT {
                    log::info!(
                        "The router's best score ({:.2}) has not improved by more than {} points since pass #{}. Stopping the auto-router after {} passes ({} item{} still unconnected).",
                        global_best_score as f64,
                        STAGNATION_SCORE_THRESHOLD,
                        pass_of_best_score,
                        current_pass,
                        incomplete_count_at_best_score,
                        if incomplete_count_at_best_score == 1 { "" } else { "s" }
                    );
                    stop.request_stop_autorouter();
                    break;
                }
            } else if after.incomplete_count == 0 && after.router_score > STAGNATION_SCORE_THRESHOLD {
                consecutive_no_improvement_passes = 0;
                last_best_score = after.router_score;
            }

            if continue_autorouting && !stop.is_stop_autorouter_requested() {
                current_pass += 1;
            }
        }

        // Finish with the best board of the history.
        let current_final = stats.score(board, settings);
        let best_history_score = bh.get_max_score();
        if best_history_score > current_final.router_score {
            if let Some(best) = bh.restore_best_board() {
                *board = best;
                let b = stats.score(board, settings);
                log::debug!(
                    "The final board state (score {}) is worse than the best board seen during routing (score {}). Restoring the best board as the final result.",
                    format_score_with_unfixable(current_final.router_score, current_final.incomplete_count, current_final.clearance_violation_count, current_final.unfixable_violation_count),
                    format_score_with_unfixable(b.router_score, b.incomplete_count, b.clearance_violation_count, b.unfixable_violation_count)
                );
            }
        }
        let was_router_run = settings.get_run_router() && settings.autorouter.max_passes.map(|m| m >= 0).unwrap_or(true);
        // Java skips the final tail removal when the autorouter was stopped (also by its own
        // stagnation rule), leaving unused fanout escapes (via + stub) on the board.
        if was_router_run && (!stop.is_stop_autorouter_requested() || ctx.enhancements) {
            self.remove_tails(board, StopConnectionOption::None, Some(stop));
        }
        if is_router_enabled {
            let s = stats.score(board, settings);
            log::info!(
                "Auto-routing stage completed: started with {} unrouted nets, completed in {:.2} seconds, final score: {}.",
                self.initial_unrouted_count,
                stage_start.elapsed().as_secs_f64(),
                format_score_with_unfixable(s.router_score, s.incomplete_count, s.clearance_violation_count, s.unfixable_violation_count)
            );
            if ignored > 0 {
                log::info!(
                    "{} of the {} unrouted connections are in ignored net classes (not routed); {} routable connection(s) left unrouted.",
                    ignored,
                    s.incomplete_count,
                    s.incomplete_count - ignored
                );
            }
            if !self.blocked.is_empty() {
                log::info!(
                    "{} item(s) cannot be routed even on the board as loaded (blocked by pins, keepouts or fixed wiring); they were skipped after their second failure.",
                    self.blocked.len()
                );
            }
        }
        bh.clear();
        !stop.is_stop_autorouter_requested()
    }
}

fn record_failure(board: &mut RoutingBoard, key: ItemKey, pass_no: i32, result: &AutorouteAttemptResult) {
    if log::log_enabled!(log::Level::Debug) {
        let item = board.item(key);
        let comp = if item.component_no() > 0 { board.components.get(item.component_no()).name.clone() } else { String::new() };
        let net = board.rules.nets.get(item.net_number(0)).map(|n| n.name.clone()).unwrap_or_default();
        let bb = item.bounding_box(board);
        let res = board.communication.resolution.max(1) as f64;
        log::debug!(
            "route failure pass {pass_no}: {} #{} {comp} net '{net}' at ({:.0}, {:.0}) um: {:?} {}",
            if item.is_pin() { "pin" } else if item.is_via() { "via" } else { "item" },
            item.id().0,
            (bb.ll.x as f64 + bb.ur.x as f64) / 2.0 / res,
            (bb.ll.y as f64 + bb.ur.y as f64) / 2.0 / res,
            result.state,
            result.details
        );
    }
    let RoutingBoard { basic, failure_log, .. } = board;
    failure_log.record_failure(basic, key, pass_no, result.state, &result.details);
}

/// Java `Item.hasIgnoredNets()`.
fn has_ignored_nets(board: &RoutingBoard, key: ItemKey) -> bool {
    board.item(key).net_numbers().iter().any(|&n| match board.rules.nets.get(n) {
        Some(net) => board.rules.net_classes[net.get_net_class()].is_ignored_by_autorouter,
        None => false,
    })
}

/// Java `BatchAutorouter.enforceStrictDrc(board, routeNetNo, maxItemIdBefore)`: if a trace or
/// via inserted after `max_item_id_before` on the net has a clearance violation, removes all
/// of them and returns the FAILED result.
pub fn enforce_strict_drc(board: &mut RoutingBoard, route_net_no: NetNo, max_item_id_before: i32) -> Option<AutorouteAttemptResult> {
    let mut new_items = Vec::new();
    let mut has_violation = false;
    for k in board.get_connectable_items(route_net_no) {
        let it = board.item(k);
        if it.id().0 <= max_item_id_before || !(it.is_trace() || it.is_via()) {
            continue;
        }
        new_items.push(k);
        if !has_violation && clearance_violation_count(&board.basic, k) > 0 {
            has_violation = true;
        }
    }
    if !has_violation {
        return None;
    }
    let n = new_items.len();
    board.remove_items(new_items);
    Some(AutorouteAttemptResult::with_details(
        AutorouteAttemptState::Failed,
        format!("strict_drc: connection ripped because {n} new item(s) included clearance violations"),
    ))
}

#[cfg(test)]
mod tests {
    use super::BatchAutorouter;
    use crate::board::{ItemKey, RoutingBoard};
    use crate::datastructures::ItemIdGenerator;
    use crate::ids::ItemId;
    use crate::library::{BoardLibrary, Packages, Padstacks};
    use crate::rules::{BoardRules, ClearanceMatrix};
    use crate::structure::{
        Communication, Components, CoordinateTransform, Layer, LayerStructure, SpecctraParserInfo, Unit,
    };
    use fr_geom::{IntBox, IntPoint, Polyline, PolylineShape, TileShape};

    fn priority(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    fn board() -> RoutingBoard {
        let ls = LayerStructure::new(vec![Layer::new("F.Cu", true), Layer::new("B.Cu", true)]);
        let matrix = ClearanceMatrix::get_default_instance(&ls, 200);
        let mut rules = BoardRules::new(ls.clone(), matrix);
        let class = rules.net_classes.append("default", &ls, false);
        for name in ["A", "B", "C"] {
            rules.nets.add(name, 1, false, class);
        }
        let padstacks = Padstacks::new(&ls);
        let comm = Communication::new(
            Unit::Mil,
            1,
            Some(SpecctraParserInfo::default()),
            CoordinateTransform::new(1.0, 0.0, 0.0),
            ItemIdGenerator::new(),
        );
        let outline = PolylineShape::Tile(TileShape::IntBox(IntBox::new(0, 0, 100_000, 100_000)));
        RoutingBoard::new(
            IntBox::new(-1000, -1000, 101_000, 101_000),
            ls,
            vec![outline],
            1,
            rules,
            BoardLibrary::new(padstacks, Packages::new()),
            Components::new(),
            comm,
        )
    }

    fn connection(board: &mut RoutingBoard, net: i32, x: i32) -> ItemKey {
        let p = Polyline::from_int_points(&[IntPoint::new(x, 0), IntPoint::new(x, 10_000)]);
        board
            .insert_trace_without_cleaning(p, 0, 100, &[net], 1, crate::ids::FixedState::Unfixed)
            .expect("trace")
    }

    fn net_numbers(board: &RoutingBoard, keys: &[ItemKey]) -> Vec<i32> {
        keys.iter().map(|&k| board.item(k).net_number(0)).collect()
    }

    #[test]
    fn net_priority_moves_listed_connections_first() {
        let mut b = board();
        let a = connection(&mut b, 1, 1000);
        let bb = connection(&mut b, 2, 2000);
        let c = connection(&mut b, 3, 3000);
        let mut list = vec![a, bb, c];
        BatchAutorouter::apply_net_priority(&b, &mut list, &[]);
        assert_eq!(net_numbers(&b, &list), vec![1, 2, 3]);
        BatchAutorouter::apply_net_priority(&b, &mut list, &priority(&["C"]));
        assert_eq!(net_numbers(&b, &list), vec![3, 1, 2]);
        // Listed nets keep the order of the list, and unlisted ones their relative order.
        let mut list = vec![c, bb, a];
        BatchAutorouter::apply_net_priority(&b, &mut list, &priority(&["A", "B"]));
        assert_eq!(net_numbers(&b, &list), vec![1, 2, 3]);
    }

    #[test]
    fn net_priority_ranks_listed_nets_in_order() {
        let p = priority(&["CLK", "RESET"]);
        assert_eq!(BatchAutorouter::rank_of_nets(["CLK"].into_iter(), &p), 0);
        assert_eq!(BatchAutorouter::rank_of_nets(["RESET"].into_iter(), &p), 1);
        assert_eq!(BatchAutorouter::rank_of_nets(["DATA"].into_iter(), &p), usize::MAX);
        // A connection carrying several nets takes the most critical of them.
        assert_eq!(BatchAutorouter::rank_of_nets(["DATA", "RESET"].into_iter(), &p), 1);
        // No list, no ranking.
        assert_eq!(BatchAutorouter::rank_of_nets(["CLK"].into_iter(), &[]), usize::MAX);
    }
}
