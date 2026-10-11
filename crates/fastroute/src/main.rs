//! `fastroute`: command-line entry point. Accepts the Freerouting CLI arguments
//! (`-de board.dsn -do board.ses -mp N --router.x.y=v ...`), routes the board (fanout,
//! autorouter, optimizer) and writes the Specctra session.
//!
//! Own options:
//! * `--parity`: deterministic Java parity mode (`scripts/java-parity.sh`): the board time
//!   limits in count mode (the deterministic call budget of the parity jar), the stage wall
//!   clock limits disabled, optimizer in `java-compat` mode (Java with `optimizer.max_threads=1`).
//! * `--time-limit-mode=wall|count|disabled` (+ `--time-limit-factor=N`, default 10): overrides
//!   the mode of the board time limits (Java `-Dfreerouting.parity.timeLimitMode`).
//! * `--optimizer-mode=java-compat|parallel`: overrides the optimizer mode (default: `parallel`
//!   when `optimizer.max_threads > 1`, else `java-compat`).
//! * `--no-time-limits`: count-mode board limits and no stage wall clock limits, without
//!   changing the optimizer mode.
//! * `--max-time=SECONDS`: stop after this wall-clock time and write the best result.
//! * `--initial-session=FILE`: start from the wiring of a session file.
//! * `--report=FILE` (+ `--diagnose`): JSON summary of the result, see [`report`].
//! * `--live[=PORT]`: live web viewer of the routing on http://127.0.0.1:PORT (default 7878),
//!   see [`live`].
//! * `-v`: debug output.
//!
//! The session file (`-do`) is also written whenever routing or optimizing reaches a new best
//! board, so a run that is stopped (Ctrl+C / SIGTERM / Ctrl+Break: stop and write the best
//! result; a second signal exits at once) or killed leaves its best result behind.

mod live;
mod report;

use std::io::Write as _;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use fr_engine::board::{TimeLimitMode, TimeLimitPolicy};
use fr_engine::datastructures::StopToken;
use fr_engine::board::{BasicBoard, ItemKey, ItemKind, RoutingBoard};
use fr_engine::pipeline::{self, CheckpointKey, OptimizerMode, PipelineContext};
use fr_settings::{available_processors, headless_merger, CliSettings, DsnFileSettings, EnvironmentSettings};

struct Args {
    design_in: Option<String>,
    design_out: Option<String>,
    parity: bool,
    no_enhancements: bool,
    multi_start: usize,
    no_time_limits: bool,
    optimizer_mode: Option<String>,
    time_limit_mode: Option<String>,
    time_limit_factor: i64,
    max_time: Option<f64>,
    tune: Option<String>,
    pairs: Option<String>,
    pair_skew: f64,
    no_neckdown_classes: Vec<String>,
    report: Option<String>,
    diagnose: bool,
    initial_session: Option<String>,
    live: Option<u16>,
    verbose: bool,
    rest: Vec<String>,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        design_in: None,
        design_out: None,
        parity: false,
        no_enhancements: false,
        multi_start: 4,
        no_time_limits: false,
        optimizer_mode: None,
        time_limit_mode: None,
        time_limit_factor: fr_engine::datastructures::time_limit::DEFAULT_COUNT_FACTOR,
        max_time: None,
        tune: None,
        pairs: None,
        pair_skew: 0.1,
        no_neckdown_classes: Vec::new(),
        report: None,
        diagnose: false,
        initial_session: None,
        live: None,
        verbose: false,
        rest: Vec::new(),
    };
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < raw.len() {
        let a = &raw[i];
        let value = || raw.get(i + 1).cloned().ok_or(format!("{a} needs a value"));
        match a.as_str() {
            "-h" | "--help" => {
                println!("fastroute {}\n{HELP}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            "-V" | "--version" => {
                println!("fastroute {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            "--parity" => {
                args.parity = true;
                i += 1;
                continue;
            }
            "--no-enhancements" => {
                args.no_enhancements = true;
                i += 1;
                continue;
            }
            _ if a.starts_with("--multi-start=") => {
                args.multi_start = a["--multi-start=".len()..]
                    .parse::<usize>()
                    .map_err(|_| format!("bad value in {a}"))?
                    .max(1);
                i += 1;
                continue;
            }
            "--diagnose" => {
                args.diagnose = true;
                i += 1;
                continue;
            }
            "--no-time-limits" => {
                args.no_time_limits = true;
                i += 1;
                continue;
            }
            "--live" => {
                args.live = Some(DEFAULT_LIVE_PORT);
                i += 1;
                continue;
            }
            "-v" => {
                args.verbose = true;
                i += 1;
                continue;
            }
            "-de" => args.design_in = Some(value()?),
            "-do" => args.design_out = Some(value()?),
            _ => {
                if let Some(m) = a.strip_prefix("--optimizer-mode=") {
                    args.optimizer_mode = Some(m.to_string());
                    i += 1;
                    continue;
                }
                if let Some(m) = a.strip_prefix("--time-limit-mode=") {
                    args.time_limit_mode = Some(m.to_string());
                    i += 1;
                    continue;
                }
                if let Some(m) = a.strip_prefix("--no-neckdown-classes=") {
                    args.no_neckdown_classes = m.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
                    i += 1;
                    continue;
                }
                if let Some(m) = a.strip_prefix("--layer-heights=") {
                    let hs: Result<Vec<f64>, _> = m.split(',').map(|x| x.trim().parse::<f64>()).collect();
                    fr_engine::tuning::set_layer_heights_mm(hs.map_err(|_| format!("bad --layer-heights '{m}' (mm, comma separated)"))?);
                    i += 1;
                    continue;
                }
                if let Some(m) = a.strip_prefix("--pairs=") {
                    args.pairs = Some(m.to_string());
                    i += 1;
                    continue;
                }
                if let Some(m) = a.strip_prefix("--pair-skew=") {
                    args.pair_skew = m.parse().map_err(|_| format!("bad --pair-skew '{m}' (mm)"))?;
                    i += 1;
                    continue;
                }
                if let Some(m) = a.strip_prefix("--initial-session=") {
                    args.initial_session = Some(m.to_string());
                    i += 1;
                    continue;
                }
                if let Some(m) = a.strip_prefix("--live=") {
                    args.live = Some(m.parse().map_err(|_| format!("bad --live port '{m}'"))?);
                    i += 1;
                    continue;
                }
                if let Some(m) = a.strip_prefix("--report=") {
                    args.report = Some(m.to_string());
                    i += 1;
                    continue;
                }
                if let Some(m) = a.strip_prefix("--tune=") {
                    args.tune = Some(m.to_string());
                    i += 1;
                    continue;
                }
                if let Some(m) = a.strip_prefix("--max-time=") {
                    let secs: f64 = m.parse().map_err(|_| format!("bad --max-time '{m}' (seconds)"))?;
                    args.max_time = Some(secs.max(1.0));
                    i += 1;
                    continue;
                }
                if let Some(m) = a.strip_prefix("--time-limit-factor=") {
                    args.time_limit_factor = m.parse().map_err(|_| format!("bad --time-limit-factor '{m}'"))?;
                    i += 1;
                    continue;
                }
            }
        }
        args.rest.push(a.clone());
        i += 1;
    }
    Ok(args)
}

const DEFAULT_LIVE_PORT: u16 = 7878;

const HELP: &str = "\
usage: fastroute -de <design.dsn> [-do <out.ses>] [options] [--router.<path>=<value> ...]

options:
  -de FILE                 input Specctra design (.dsn)
  -do FILE                 output Specctra session (.ses); also rewritten with the best
                           board so far whenever routing/optimizing improves
  -mp N                    maximum autorouter passes (router.autorouter.max_passes)
  --no-neckdown-classes=A,B  keep the full trace width of these net classes at pins
                           (controlled impedance)
  --pairs=FILE             differential pairs: routed first, N running along P (see below);
                           afterwards the shorter net of each pair gets meanders until the
                           lengths differ by at most --pair-skew (default 0.1 mm; 0 = off)
  --layer-heights=MM,..    height of each copper layer from the top (stackup), so that via
                           lengths count in length matching as in KiCad
  --tune=FILE              length matching after routing (groups of nets, see below)
  --max-time=SECONDS       stop after this wall-clock time and write the best result
                           (Ctrl+C / SIGTERM / Ctrl+Break do the same; a second one exits)
  --initial-session=FILE   start from the wiring of a session (.ses), e.g. a checkpoint of an
                           earlier run, instead of the wiring in the design
  --report=FILE            write a JSON summary: statistics, timings, unrouted connections
                           and clearance violations
  --diagnose               with --report: route every unrouted connection alone on the
                           loaded board (congestion vs. blocked by geometry/rules)
  --live[=PORT]            watch the routing live in the browser (http://127.0.0.1:PORT,
                           default 7878; opened automatically)
  --multi-start=N          rerun the autorouter with N-1 shuffled orders in parallel if
                           connections stay unrouted (default 4; skipped after a first run
                           longer than 10 minutes)
  --no-enhancements        Freerouting's behaviour without fastroute's improvements
  --parity                 byte-identical Freerouting results (deterministic, slower)
  --optimizer-mode=M       java-compat | parallel
  --time-limit-mode=M      wall | count | disabled (board time limits)
  --time-limit-factor=N    count-mode budget factor (default 10)
  --no-time-limits         deterministic limits, no stage wall-clock limits
  -v                       debug output

tune file: one group per `group` line, followed by net names (* = any text):
  group sdram_data tolerance=0.5            # match to the longest net, -0.5 mm allowed
    /SD_D*
    /SD_NBL*
  group sdram_clk tolerance=0.2 target=40   # fixed target length in mm

pairs file: one pair per line, optional copper gap in mm (default: their clearance),
also per layer, and the allowed length difference (skew=, default --pair-skew). Pairs are
routed and coupled first, then fixed while the rest is routed:
  pair /USB_DP /USB_DN gap=0.15 gap@In1.Cu=0.12 skew=0.05

common --router.* settings (numbers, true/false, comma-separated lists):
  --router.autorouter.max_passes=N          passes (0 = unlimited)
  --router.autorouter.min_passes=N          the stagnation rules do not stop the autorouter
                                            before N passes (long runs with --max-time)
  --router.autorouter.ignore_net_classes=A,B  net classes left unrouted
  --router.autorouter.max_threads=N         threads of the parallel autorouting pass
                                            (1 = sequential pass as in Freerouting)
  --router.optimizer.enabled=true|false     run the optimizer
  --router.optimizer.max_threads=N          optimizer threads
  --router.optimizer.optimization_improvement_threshold=P  stop below P % per pass
  --router.fanout.enabled=true|false        SMD fanout stage
  --router.scoring.via_costs=N, --router.scoring.plane_via_costs=N
  --router.min_trace_width_um=W             never neck traces below W um
  --router.neck_width_um=W                  retry failed connections with W um traces
  --router.copper_to_edge_clearance_um=C    copper to board edge clearance
  --router.job_timeout=HH:MM:SS             overall job timeout (default 12:00:00)";


/// Minimal stderr logger: pipeline progress (info) and warnings/errors of all modules.
struct Logger {
    start: Instant,
    verbose: bool,
}

impl log::Log for Logger {
    fn enabled(&self, m: &log::Metadata) -> bool {
        let ours = m.target().starts_with("fr_engine::pipeline") || m.target().starts_with("fastroute") || m.target() == "parity";
        match m.level() {
            log::Level::Error => true,
            // (the engine warns like the Java FRLogger, e.g. about degenerate polylines)
            log::Level::Warn => ours || self.verbose,
            log::Level::Info => ours || self.verbose,
            _ => self.verbose && (m.target().starts_with("fr_engine::pipeline") || m.target().starts_with("fastroute")),
        }
    }
    fn log(&self, r: &log::Record) {
        if !self.enabled(r.metadata()) {
            return;
        }
        let t = self.start.elapsed().as_secs_f64();
        let level = match r.level() {
            log::Level::Info => "INFO ",
            log::Level::Warn => "WARN ",
            log::Level::Error => "ERROR",
            _ => "DEBUG",
        };
        let _ = writeln!(std::io::stderr().lock(), "{t:9.3} {level} {}", r.args());
        if let Some(live) = LIVE.get() {
            live.log(level, &r.args().to_string());
        }
    }
    fn flush(&self) {}
}

static LOGGER: OnceLock<Logger> = OnceLock::new();
/// The live viewer (`--live`), also fed with the log lines.
static LIVE: OnceLock<Arc<live::Live>> = OnceLock::new();
/// The stage the pipeline is in (named in the messages after a stop request).
static STAGE: Mutex<String> = Mutex::new(String::new());
/// The last progress event of the stage (named with the stage after a stop request).
static STEP: Mutex<String> = Mutex::new(String::new());

/// After a stop request: says every half minute that the run is still finishing its current
/// step, so that a long step on a big board is not taken for a hang.
fn stop_watchdog() {
    std::thread::spawn(|| {
        let t = Instant::now();
        loop {
            std::thread::sleep(Duration::from_secs(30));
            let stage = STAGE.lock().map(|s| s.clone()).unwrap_or_default();
            let step = STEP.lock().map(|s| s.clone()).unwrap_or_default();
            log::warn!(
                target: "fastroute",
                "still finishing the current step of the {} stage{} ({:.0} s since the stop request); the session file is written when it ends (the checkpoint file holds the best board so far)",
                if stage.is_empty() { "loading" } else { stage.as_str() },
                if step.is_empty() { String::new() } else { format!(" (last progress: {step})") },
                t.elapsed().as_secs_f64()
            );
        }
    });
}

/// Opens `url` in the default browser (best effort).
fn open_browser(url: &str) {
    let cmd = if cfg!(target_os = "macos") {
        ("open", vec![url])
    } else if cfg!(windows) {
        ("cmd", vec!["/C", "start", "", url])
    } else {
        ("xdg-open", vec![url])
    };
    let _ = std::process::Command::new(cmd.0)
        .args(cmd.1)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    let logger = LOGGER.get_or_init(|| Logger { start: Instant::now(), verbose: args.verbose });
    let _ = log::set_logger(logger);
    log::set_max_level(if args.verbose { log::LevelFilter::Debug } else { log::LevelFilter::Info });

    log::info!(target: "fastroute", "fastroute {}", env!("CARGO_PKG_VERSION"));
    let stop = StopToken::new();
    if let Some(port) = args.live {
        let (live, url) = live::Live::start(port, stop.clone(), logger.start)?;
        let _ = LIVE.set(live);
        log::info!(target: "fastroute", "live viewer: {url}");
        if std::env::var_os("FASTROUTE_LIVE_NO_OPEN").is_none() {
            open_browser(&url);
        }
    }
    let cli = CliSettings::parse(&args.rest);
    for w in &cli.warnings {
        log::warn!("{w}");
    }
    let env = EnvironmentSettings::from_process_env();
    let procs = available_processors();

    let Some(path) = args.design_in else {
        return Err("no input design given (-de board.dsn)".into());
    };
    let t_start = Instant::now();
    let t = Instant::now();
    let data = std::fs::read(&path).map_err(|e| format!("{path}: {e}"))?;
    let dsn = fr_dsn::Dsn::parse(&data).map_err(|e| format!("{path}: {e}"))?;
    for w in &dsn.warnings {
        log::warn!("{w}");
    }
    let dsn_settings = DsnFileSettings::from_dsn(&dsn);
    let mut settings = headless_merger(&cli, &env, Some(&dsn_settings), None, procs).merge(procs);
    log::info!(
        target: "fastroute",
        "parsed '{}' in {:.1} ms: {} layers, {} components, {} nets, {} wires",
        dsn.name,
        t.elapsed().as_secs_f64() * 1e3,
        dsn.structure.layers.len(),
        dsn.placement.iter().map(|c| c.places.len()).sum::<usize>(),
        dsn.network.nets.len(),
        dsn.wiring.wires.len(),
    );

    // HeadlessBoardManager.loadFromSpecctraDsn + RoutingJobScheduler preparation.
    let t = Instant::now();
    let mut board = fr_io::post_load::load_from_specctra_dsn(&data, &mut settings).map_err(|e| format!("{path}: {e:?}"))?;
    if !args.parity && !args.no_enhancements {
        // clearance-report fixes (docs/IMPROVEMENTS.md): exact shapes instead of bounding octagons,
        // a footprint's own pads not judged against each other
        let rules = board.rules_mut();
        rules.exact_clearance_check = true;
        rules.skip_same_component_pin_pairs = true;
        let n = fr_io::network::extend_class_pair_clearances(&mut board, &dsn);
        if n > 0 {
            log::info!(target: "fastroute", "class-pair clearances applied to {n} pin/SMD clearance class pairs as well");
        }
    }
    if !args.no_neckdown_classes.is_empty() {
        let rules = board.rules_mut();
        let ids: Vec<_> = rules.net_classes.iter().collect();
        for c in ids {
            let name = rules.net_classes[c].get_name().to_string();
            let hit = args.no_neckdown_classes.iter().any(|n| {
                n.eq_ignore_ascii_case(&name) || (n.eq_ignore_ascii_case("Default") && fr_io::network::is_kicad_default_net_class_name(&name))
            });
            if hit {
                rules.net_classes[c].no_neckdown = true;
                log::info!(target: "fastroute", "net class '{name}': no neck-down");
            }
        }
    }
    let initial = match &args.initial_session {
        Some(f) => Some(std::fs::read(f).map_err(|e| format!("{f}: {e}"))?),
        None => None,
    };
    let wiring_ids = |b: &RoutingBoard| -> std::collections::HashSet<fr_engine::ids::ItemId> {
        b.get_items().into_iter().filter(|&k| b.item(k).is_trace() || b.item(k).is_via()).map(|k| b.item(k).id()).collect()
    };
    let before = wiring_ids(&board);
    if let Some(summary) = fr_io::post_load::prepare_for_routing(&mut board, &mut settings, initial.as_deref()) {
        // the session reader fixes what it imports (as Freerouting does); here the session is
        // a starting point that may be ripped and optimized like routed wiring
        let imported: Vec<_> = wiring_ids(&board).difference(&before).copied().collect();
        fr_engine::diffpair::release_pairs(&mut board, &imported);
        log::info!(
            target: "fastroute",
            "initial session '{}': {} wires and {} vias imported ({} errors)",
            args.initial_session.as_deref().unwrap_or(""),
            summary.wires_imported,
            summary.vias_imported,
            summary.errors_encountered
        );
    }
    if !args.parity && !args.no_enhancements {
        // (before the pre-existing violations are counted: wiring of the design outside the
        // outline is the design's, not the router's)
        let (pins, pieces) = board.keep_wiring_inside_outline();
        if pieces > 0 {
            log::info!(target: "fastroute", "{pins} pins on or outside the board outline: wiring kept inside the outline except at these pins");
        }
    }
    pipeline::deferred_post_load_processing(&mut board);
    if args.parity || args.no_enhancements {
        board.wire_keepouts_as_keepouts();
    }
    if !args.parity && !args.no_enhancements {
        board.fallback_vias_own_class = true;
        board.set_overlap_contacts(true);
        let n = board.mark_stitching_vias();
        if n > 0 {
            log::info!(target: "fastroute", "{n} fixed vias without a connection (zone stitching vias): kept, treated as joined by their zone");
        }
        let n = board.bridge_trace_ends_to_drill_centers();
        if n > 0 {
            log::info!(target: "fastroute", "{n} trace ends inside a pad but off its center joined to the center");
        }
    }
    log::info!(
        target: "fastroute",
        "built board in {:.1} ms: {} items ({} pins, {} vias, {} traces)",
        t.elapsed().as_secs_f64() * 1e3,
        board.get_items().len(),
        board.get_pins().len(),
        board.get_vias().len(),
        board.get_traces().len(),
    );

    // Time limits and optimizer mode.
    let limits = !(args.parity || args.no_time_limits);
    let fired = Arc::new(AtomicBool::new(false));
    let mode = match args.time_limit_mode.as_deref() {
        Some("wall") => TimeLimitMode::WallClock,
        Some("count") => TimeLimitMode::Count { factor: args.time_limit_factor },
        Some("disabled") => TimeLimitMode::Disabled,
        Some(m) => return Err(format!("unknown time limit mode '{m}' (wall, count or disabled)")),
        None if limits => TimeLimitMode::WallClock,
        None => TimeLimitMode::Count { factor: args.time_limit_factor },
    };
    board.time_limits = TimeLimitPolicy { mode, fired: Some(fired.clone()) };
    let optimizer_threads = settings.optimizer.max_threads.unwrap_or(1).max(1) as usize;
    let optimizer_mode = match args.optimizer_mode.as_deref() {
        Some("java-compat") => OptimizerMode::JavaCompat,
        Some("parallel") => OptimizerMode::Parallel { threads: optimizer_threads },
        Some(m) => return Err(format!("unknown optimizer mode '{m}' (java-compat or parallel)")),
        None if args.parity || optimizer_threads <= 1 => OptimizerMode::JavaCompat,
        None => OptimizerMode::Parallel { threads: optimizer_threads },
    };
    let design_name = std::path::Path::new(&path).file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let checkpoint: Option<pipeline::Checkpoint> = args.design_out.clone().map(|out| {
        let best: std::sync::Mutex<Option<CheckpointKey>> = std::sync::Mutex::new(None);
        let name = design_name.clone();
        Arc::new(move |board: &RoutingBoard, key: CheckpointKey| {
            let mut best = best.lock().unwrap();
            if best.as_ref().is_some_and(|b| !key.better_than(b)) {
                return;
            }
            match write_ses_atomically(&out, &fr_io::ses_writer::ses_bytes(board, &name)) {
                Ok(()) => {
                    *best = Some(key);
                    log::info!(
                        target: "fastroute",
                        "checkpoint: best board so far ({} unrouted, {} violations) written to '{out}'",
                        key.incomplete,
                        key.violations
                    );
                }
                Err(e) => log::warn!(target: "fastroute", "checkpoint: cannot write '{out}': {e}"),
            }
        }) as pipeline::Checkpoint
    });
    let ctx = PipelineContext { stop, wall_clock_limits: limits, optimizer_mode,
        enhancements: !args.parity && !args.no_enhancements,
        multi_start: args.multi_start,
        checkpoint,
        observer: Some(Arc::new(|board, ev| {
            match ev {
                pipeline::LiveEvent::Stage(name) => {
                    if let Ok(mut s) = STAGE.lock() {
                        *s = name.to_string();
                    }
                    if let Ok(mut s) = STEP.lock() {
                        s.clear();
                    }
                }
                pipeline::LiveEvent::Connection { pass_no, done, total, .. } => {
                    if let Ok(mut s) = STEP.lock() {
                        *s = format!("pass {pass_no}, {done} of {total} connections committed");
                    }
                }
                pipeline::LiveEvent::Fanout { pass_no, done, total, .. } => {
                    if let Ok(mut s) = STEP.lock() {
                        *s = format!("fanout pass {pass_no}, {done} of {total} pins checked");
                    }
                }
                _ => {}
            }
            if let Some(live) = LIVE.get() {
                live.observe(board, ev);
            }
        })),
    };
    if let Some(live) = LIVE.get() {
        live.publish_board(&board, &design_name);
    }
    {
        // First signal: stop and write the best result; second: exit at once.
        let stop = ctx.stop.clone();
        let signalled = AtomicBool::new(false);
        let _ = ctrlc::set_handler(move || {
            if signalled.swap(true, Ordering::SeqCst) {
                eprintln!("fastroute: second stop signal, exiting");
                std::process::exit(130);
            }
            log::warn!(target: "fastroute", "stop requested: finishing with the best result so far");
            stop.request_stop();
            stop_watchdog();
        });
    }
    if let Some(secs) = args.max_time {
        let stop = ctx.stop.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs_f64(secs));
            log::warn!(target: "fastroute", "--max-time ({secs:.0} s) reached: finishing with the best result so far");
            stop.request_stop();
            stop_watchdog();
        });
    }
    log::info!(
        target: "fastroute",
        "threads: autorouting passes run on router.autorouter.max_threads threads (1 = sequential); the optimizer uses router.optimizer.max_threads"
    );

    log::info!(
        target: "fastroute",
        "optimizer mode: {optimizer_mode:?}, board time limits: {mode:?}, stage wall clock limits: {}",
        if limits { "enabled" } else { "disabled" }
    );

    // Job timeout (RoutingJobSchedulerActionThread monitor thread).
    if limits {
        if let Some(secs) = settings.job_timeout_string.as_deref().and_then(fr_settings::parse_timespan_string) {
            let secs = secs.min(24 * 3600);
            let stop = ctx.stop.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(secs.max(0) as u64));
                log::warn!("job timeout ({secs} s) reached, stopping");
                stop.request_stop();
            });
        }
    }

    let pairs = match &args.pairs {
        Some(f) => parse_pairs_file(f)?,
        None => Vec::new(),
    };
    let tune_groups = match &args.tune {
        Some(f) => parse_tune_file(f)?,
        None => Vec::new(),
    };
    // differential pairs are routed and coupled first, then fixed while the rest is routed
    let mut pair_items = Vec::new();
    if !pairs.is_empty() {
        let t = Instant::now();
        // how the pre-routed pairs are held while the rest is routed (experiment switch)
        let hold = match std::env::var("FASTROUTE_PAIR_HOLD").as_deref() {
            Ok("none") => None,
            Ok("shove") => Some(fr_engine::ids::FixedState::ShoveFixed),
            _ => Some(fr_engine::ids::FixedState::UserFixed),
        };
        ctx.observe(&board, &pipeline::LiveEvent::Stage("diff pairs"));
        let (results, fixed) = fr_engine::diffpair::preroute_pairs(&mut board, &pairs, &settings, &ctx.stop, hold);
        for r in &results {
            log_pair(r, "pre-routed");
        }
        log::info!(target: "fastroute", "diff pairs pre-routed in {:.2} s ({} items fixed during routing)", t.elapsed().as_secs_f64(), fixed.len());
        pair_items = fixed;
    }
    let load_s = t_start.elapsed().as_secs_f64();
    // the board as loaded, for routing unrouted connections alone afterwards (--diagnose)
    let loaded = (args.report.is_some() && args.diagnose).then(|| board.clone());
    let t = Instant::now();
    pipeline::run_pipeline(&mut board, &mut settings, &ctx);
    if !pair_items.is_empty() {
        fr_engine::diffpair::release_pairs(&mut board, &pair_items);
        // the fixed pairs may have blocked connections: route once more with the pairs free
        // (the autorouter rips them only where it has to; the coupling is retried afterwards)
        let unrouted = fr_engine::pipeline::stats::incomplete_count(&board, None);
        if unrouted > 0 && !ctx.stop.is_stop_requested() {
            log::info!(target: "fastroute", "diff pairs released: {unrouted} connections unrouted with the pairs fixed, routing again");
            pipeline::run_pipeline(&mut board, &mut settings, &ctx);
        }
    }
    let route_s = t.elapsed().as_secs_f64();
    log::info!(target: "fastroute", "routing finished in {route_s:.2} s");
    if fired.load(Ordering::SeqCst) {
        log::info!(target: "fastroute", "note: a board time limit fired ({mode:?})");
    }

    if !pairs.is_empty() && !ctx.stop.is_stop_requested() {
        let t = Instant::now();
        for r in fr_engine::diffpair::couple_pairs(&mut board, &pairs, &settings, &ctx.stop) {
            log_pair(&r, "after routing");
        }
        for r in fr_engine::diffpair::match_pair_lengths(&mut board, &pairs, &settings, args.pair_skew, &ctx.stop) {
            log::info!(
                target: "fastroute",
                "diff pair {} / {}: length difference {:+.3} -> {:+.3} mm (tolerance {:.2}){}{}",
                r.p,
                r.n,
                r.skew_before_mm,
                r.skew_after_mm,
                r.tolerance_mm,
                if r.on_coupled_mm > 0.0 { format!(", {:.2} mm of meanders on the coupled runs", r.on_coupled_mm) } else { String::new() },
                if r.message.is_empty() { String::new() } else { format!(": {}", r.message) }
            );
        }
        log::info!(target: "fastroute", "diff pairs finished in {:.2} s", t.elapsed().as_secs_f64());
    }
    if !tune_groups.is_empty() && !ctx.stop.is_stop_requested() {
        let t = Instant::now();
        ctx.observe(&board, &pipeline::LiveEvent::Stage("length tuning"));
        let results = fr_engine::tuning::tune_lengths(&mut board, &tune_groups);
        for g in &results {
            let (min, max) = g.nets.iter().fold((f64::MAX, 0.0f64), |(lo, hi), n| (lo.min(n.after_mm), hi.max(n.after_mm)));
            let min_before = g.nets.iter().map(|n| n.before_mm).fold(f64::MAX, f64::min);
            log::info!(
                target: "fastroute",
                "length tuning '{}': {} nets, target {:.2} mm (-{:.2}), before {:.2}..{:.2} mm, after {:.2}..{:.2} mm, {} still short",
                g.name,
                g.nets.len(),
                g.target_mm,
                g.tolerance_mm,
                min_before,
                g.nets.iter().map(|n| n.before_mm).fold(0.0, f64::max),
                min,
                max,
                g.short_nets()
            );
            for n in &g.nets {
                let short = n.after_mm < g.target_mm - g.tolerance_mm - 1e-6;
                log::debug!(target: "fastroute", "  {:20} {:8.2} -> {:8.2} mm{}", n.net, n.before_mm, n.after_mm, if short { "  (short)" } else { "" });
                if short {
                    log::warn!(target: "fastroute", "length tuning '{}': {} is {:.2} mm, {:.2} mm short (no room for meanders)", g.name, n.net, n.after_mm, g.target_mm - g.tolerance_mm - n.after_mm);
                }
            }
        }
        log::info!(target: "fastroute", "length tuning finished in {:.2} s", t.elapsed().as_secs_f64());
    }
    report_violations(&board);
    if let Some(out) = args.design_out {
        // The Java CLI names the session after the input file stem.
        let bytes = fr_io::ses_writer::ses_bytes(&board, &design_name);
        write_ses_atomically(&out, &bytes).map_err(|e| format!("{out}: {e}"))?;
        log::info!(target: "fastroute", "saved '{out}' ({} bytes)", bytes.len());
    }
    if let Some(out) = &args.report {
        let timings = report::Timings { load_s, route_s, total_s: t_start.elapsed().as_secs_f64() };
        let unclamped = !args.parity && !args.no_enhancements;
        report::write_report(out, &path, &board, loaded.as_ref(), &settings, unclamped, unclamped, &timings).map_err(|e| format!("{out}: {e}"))?;
        log::info!(target: "fastroute", "report written to '{out}'");
    }
    if let Some(live) = LIVE.get() {
        live.finish(&board, Duration::from_secs(3));
    }
    Ok(())
}

/// Writes via a temporary file and a rename, so a reader (or a kill) never sees half a file.
fn write_ses_atomically(out: &str, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = format!("{out}.tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, out)
}

/// Lists the clearance violations of the result (up to 50) with location and items.
fn report_violations(board: &RoutingBoard) {
    let violations = fr_engine::drc::all_clearance_violations(board);
    if violations.is_empty() {
        return;
    }
    let res = board.communication.resolution.max(1) as f64;
    let layer_name =
        |l: i32| board.layer_structure.layers.get(l as usize).map(|x| x.name.clone()).unwrap_or_else(|| format!("L{l}"));
    log::warn!(target: "fastroute", "{} clearance violation(s) in the result:", violations.len());
    for v in violations.iter().take(50) {
        let b = v.shape.bounding_box();
        log::warn!(
            target: "fastroute",
            "  {} at ({:.3}, {:.3}) mm: clearance {:.3} mm, actual {:.3} mm{}: {} / {}",
            layer_name(v.layer),
            (b.ll.x + b.ur.x) as f64 / 2.0 / res / 1000.0,
            -((b.ll.y + b.ur.y) as f64) / 2.0 / res / 1000.0,
            v.expected_clearance / res / 1000.0,
            v.actual_clearance / res / 1000.0,
            if v.is_unfixable(board) { " (pre-existing, unfixable)" } else { "" },
            describe_item(board, v.first_item),
            describe_item(board, v.second_item)
        );
    }
}

fn describe_item(board: &BasicBoard, key: ItemKey) -> String {
    let item = board.item(key);
    let kind = match &item.kind {
        ItemKind::Pin(_) => "pin",
        ItemKind::Via(_) => "via",
        ItemKind::Trace(_) => "trace",
        ItemKind::ObstacleArea(_) => "keepout",
        ItemKind::ConductionArea(_) => "plane",
        ItemKind::ComponentOutline(_) => "outline",
        ItemKind::BoardOutline(_) => "board outline",
    };
    let nets: Vec<String> =
        item.net_numbers().iter().map(|n| board.rules.nets.get(*n).map(|n| n.name.clone()).unwrap_or_default()).collect();
    let comp = if item.component_no() > 0 { format!(" {}", board.components.get(item.component_no()).name) } else { String::new() };
    format!("{kind}{comp} [{}]", nets.join(","))
}


/// mimalloc is noticeably faster than the system allocator for the many small, short-lived
/// allocations of the geometry code (see docs/PERFORMANCE.md).
#[cfg(not(feature = "dhat-heap"))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[cfg(feature = "dhat-heap")]
#[global_allocator]
static GLOBAL: dhat::Alloc = dhat::Alloc;

fn main() -> ExitCode {
    #[cfg(feature = "dhat-heap")]
    let _profiler = dhat::Profiler::new_heap();
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("fastroute: {e}");
            ExitCode::FAILURE
        }
    }
}

fn log_pair(r: &fr_engine::diffpair::PairResult, stage: &str) {
    log::info!(
        target: "fastroute",
        "diff pair {}/{} ({stage}): coupled {:.2} -> {:.2} mm of {:.2} mm, skew {:+.2} mm{}",
        r.p,
        r.n,
        r.coupled_before_mm,
        r.coupled_after_mm,
        r.p_length_mm,
        r.skew_mm,
        if r.message.is_empty() { String::new() } else { format!(" ({})", r.message) }
    );
}

/// Reads a `--pairs` file (see the help text).
fn parse_pairs_file(path: &str) -> Result<Vec<fr_engine::diffpair::DiffPair>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    let mut pairs = Vec::new();
    for (no, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let words: Vec<&str> = line.split_whitespace().collect();
        if words.len() < 3 || words[0] != "pair" {
            return Err(format!("{path}:{}: expected 'pair P N [gap=MM] [skew=MM]'", no + 1));
        }
        let mut pair = fr_engine::diffpair::DiffPair { p: words[1].to_string(), n: words[2].to_string(), gap_mm: None, layer_gaps_mm: Vec::new(), skew_mm: None };
        for w in &words[3..] {
            let (k, v) = w.split_once('=').ok_or(format!("{path}:{}: unknown option '{w}'", no + 1))?;
            let mm: f64 = v.trim_end_matches("mm").parse().map_err(|_| format!("{path}:{}: bad number '{v}'", no + 1))?;
            match k.split_once('@') {
                None if k == "gap" => pair.gap_mm = Some(mm),
                None if k == "skew" => pair.skew_mm = Some(mm),
                Some(("gap", layer)) => pair.layer_gaps_mm.push((layer.to_string(), mm)),
                _ => return Err(format!("{path}:{}: unknown option '{w}'", no + 1)),
            }
        }
        pairs.push(pair);
    }
    Ok(pairs)
}

/// Reads a `--tune` file (see the help text).
fn parse_tune_file(path: &str) -> Result<Vec<fr_engine::tuning::TuneGroup>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    let mut groups: Vec<fr_engine::tuning::TuneGroup> = Vec::new();
    for (no, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let mut words = line.split_whitespace();
        if line.starts_with("group ") {
            words.next();
            let name = words.next().ok_or(format!("{path}:{}: group needs a name", no + 1))?.to_string();
            let mut g = fr_engine::tuning::TuneGroup { name, nets: Vec::new(), tolerance_mm: 0.5, target_mm: None };
            for w in words {
                let (k, v) = w.split_once('=').ok_or(format!("{path}:{}: expected key=value, got '{w}'", no + 1))?;
                let v: f64 = v.trim_end_matches("mm").parse().map_err(|_| format!("{path}:{}: bad number '{v}'", no + 1))?;
                match k {
                    "tolerance" => g.tolerance_mm = v,
                    "target" => g.target_mm = Some(v),
                    _ => return Err(format!("{path}:{}: unknown key '{k}'", no + 1)),
                }
            }
            groups.push(g);
        } else {
            let g = groups.last_mut().ok_or(format!("{path}:{}: net before the first group line", no + 1))?;
            g.nets.extend(words.map(str::to_string));
        }
    }
    Ok(groups)
}
