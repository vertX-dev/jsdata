//! `--watch`: rebuild on config/source change with a ~200 ms debounce.
//! Only entries whose source actually changed are re-extracted; the module
//! is then re-emitted in full (and skipped if byte-identical).

use crate::config::{Config, Entry};
use crate::{emit, entry_version, resolve_langs_entry, tdz_warnings};
use notify::{EventKind, RecursiveMode, Watcher};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

/// What a change must touch to re-resolve an entry.
enum Trigger {
    Static,                                // Let — never file-driven
    File(PathBuf),                         // Var — canonical source path
    Langs { dir: PathBuf, json: PathBuf }, // Langs — texts dir + languages.json
}

struct State {
    cfg: Config,
    cfg_errors: Vec<String>,
    // per entry, config order. `Ok(None)` = declared but absent at the
    // active version — skipped with a warning, not an error.
    values: Vec<Result<Option<String>, String>>,
    warns: Vec<Vec<String>>,             // per entry (langs missing-locale etc.)
    triggers: Vec<Trigger>,              // per entry, what change re-resolves it
    cli_version: crate::version::VersionSpec, // command-line --version/--tag
    display: String,                     // config path as given, for the header
}

impl State {
    fn new(config_path: &Path, cli_version: crate::version::VersionSpec) -> Self {
        let display = config_path.display().to_string();
        let (cfg, cfg_errors) = match crate::load(config_path) {
            Ok(v) => v,
            Err(e) => (
                Config {
                    out: PathBuf::new(),
                    entries: vec![],
                    version: None,
                },
                vec![e],
            ),
        };
        let cfg_ver = cfg.version.clone();
        let (values, warns): (Vec<_>, Vec<_>) = cfg
            .entries
            .iter()
            .map(|e| {
                let eff = crate::effective_version(&[
                    entry_version(e),
                    Some(&cli_version),
                    cfg_ver.as_ref(),
                ]);
                resolve_langs_entry(e, eff.as_ref())
            })
            .unzip();
        let triggers = cfg
            .entries
            .iter()
            .map(|e| match e {
                Entry::Var { source, .. } | Entry::Md { source, .. } => {
                    source.canonicalize().ok().map_or(Trigger::Static, Trigger::File)
                }
                Entry::Langs { texts_dir, languages_json, .. } => Trigger::Langs {
                    dir: texts_dir.canonicalize().unwrap_or_else(|_| texts_dir.clone()),
                    json: languages_json.canonicalize().unwrap_or_else(|_| languages_json.clone()),
                },
                Entry::Let { .. } => Trigger::Static,
            })
            .collect();
        State {
            cfg,
            cfg_errors,
            values,
            warns,
            triggers,
            cli_version,
            display,
        }
    }

    /// Re-resolve entries a change touched; count refreshed. A langs entry
    /// refreshes when any changed path is its `languages.json` or lives under
    /// its texts dir.
    fn refresh(&mut self, changed: &HashSet<PathBuf>) -> usize {
        let hits: Vec<usize> = self
            .triggers
            .iter()
            .enumerate()
            .filter_map(|(i, trig)| {
                let hit = match trig {
                    Trigger::Static => false,
                    Trigger::File(p) => changed.contains(p),
                    Trigger::Langs { dir, json } => {
                        changed.iter().any(|c| c == json || c.starts_with(dir))
                    }
                };
                hit.then_some(i)
            })
            .collect();
        let cfg_ver = self.cfg.version.as_ref();
        for &i in &hits {
            let entry = &self.cfg.entries[i];
            let eff = crate::effective_version(&[
                entry_version(entry),
                Some(&self.cli_version),
                cfg_ver,
            ]);
            let (res, warns) = resolve_langs_entry(entry, eff.as_ref());
            self.values[i] = res;
            self.warns[i] = warns;
        }
        hits.len()
    }

    fn emit(&self) {
        for w in self.warns.iter().flatten() {
            eprintln!("warning: {w}");
        }
        for w in tdz_warnings(&self.cfg) {
            eprintln!("warning: {w}");
        }
        let mut errors = self.cfg_errors.clone();
        let mut ok = Vec::new();
        for (e, v) in self.cfg.entries.iter().zip(&self.values) {
            match v {
                Ok(Some(js)) => ok.push((e.name().to_string(), js.clone())),
                Ok(None) => eprintln!(
                    "warning: {}: not present at this version — skipped",
                    e.name()
                ),
                Err(err) => errors.push(err.clone()),
            }
        }
        if !errors.is_empty() {
            for e in &errors {
                eprintln!("error: {e}");
            }
            eprintln!("not writing {} ({} error(s))", nice(&self.cfg.out), errors.len());
            return;
        }
        let content = emit::render(&self.display, &ok);
        match emit::write_if_changed(&self.cfg.out, &content) {
            Ok(true) => println!("wrote {}", nice(&self.cfg.out)),
            Ok(false) => println!("unchanged {}", nice(&self.cfg.out)),
            Err(e) => eprintln!("error: cannot write {}: {e}", nice(&self.cfg.out)),
        }
    }
}

pub fn watch(config_path: &Path, version: Option<String>) -> Result<(), String> {
    watch_with(config_path, &crate::Options::from_spec(version.as_deref()))
}

/// [`watch`] with the full command-line [`crate::Options`].
pub fn watch_with(config_path: &Path, opts: &crate::Options) -> Result<(), String> {
    let version = opts.version.clone();
    // canonical path only for event comparison; keep the given path for
    // loading and display
    let canon_cfg = config_path
        .canonicalize()
        .map_err(|e| format!("cannot resolve {}: {e}", config_path.display()))?;

    let (tx, rx) = mpsc::channel::<Vec<PathBuf>>();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(ev) = res {
            if matches!(
                ev.kind,
                EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
            ) {
                let _ = tx.send(ev.paths);
            }
        }
    })
    .map_err(|e| e.to_string())?;

    let mut state = State::new(config_path, version.clone());
    state.emit();
    let mut watched: HashSet<PathBuf> = HashSet::new();
    rewatch(&mut watcher, &mut watched, &canon_cfg, &state.cfg);
    eprintln!("watching (Ctrl-C to stop)...");

    loop {
        let first = rx.recv().map_err(|_| "watcher channel closed".to_string())?;
        let mut changed: HashSet<PathBuf> = HashSet::new();
        changed.extend(first.into_iter().filter_map(|p| p.canonicalize().ok()));
        // debounce: keep draining until ~200 ms of quiet
        while let Ok(more) = rx.recv_timeout(Duration::from_millis(200)) {
            changed.extend(more.into_iter().filter_map(|p| p.canonicalize().ok()));
        }
        if changed.contains(&canon_cfg) {
            eprintln!("config changed - full rebuild");
            state = State::new(config_path, version.clone());
            state.emit();
            rewatch(&mut watcher, &mut watched, &canon_cfg, &state.cfg);
        } else if state.refresh(&changed) > 0 {
            state.emit();
        }
        // ponytail: a deleted-then-recreated source misses one event round
        // (canonicalize fails while absent); the next save picks it up
    }
}

/// Watch parent directories, not files — editors replace files on save,
/// which breaks direct file watches.
fn rewatch(
    watcher: &mut notify::RecommendedWatcher,
    watched: &mut HashSet<PathBuf>,
    canon_cfg: &Path,
    cfg: &Config,
) {
    let mut want: HashSet<PathBuf> = HashSet::new();
    if let Some(d) = canon_cfg.parent() {
        want.insert(d.to_path_buf());
    }
    // canonical parent directory of a file (fall back to cwd for a bare name)
    fn dir_of(file: &Path) -> PathBuf {
        let dir = file
            .parent()
            .filter(|d| !d.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .to_path_buf();
        dir.canonicalize().unwrap_or(dir)
    }
    for e in &cfg.entries {
        match e {
            Entry::Var { source, .. } | Entry::Md { source, .. } => {
                want.insert(dir_of(source));
            }
            Entry::Langs { texts_dir, languages_json, .. } => {
                // the texts dir itself holds the .lang files; also cover
                // languages.json's dir in case it lives elsewhere
                want.insert(texts_dir.canonicalize().unwrap_or_else(|_| texts_dir.clone()));
                want.insert(dir_of(languages_json));
            }
            Entry::Let { .. } => {}
        }
    }
    for stale in watched.difference(&want).cloned().collect::<Vec<_>>() {
        let _ = watcher.unwatch(&stale);
        watched.remove(&stale);
    }
    for fresh in want.difference(watched).cloned().collect::<Vec<_>>() {
        match watcher.watch(&fresh, RecursiveMode::NonRecursive) {
            Ok(()) => {
                watched.insert(fresh);
            }
            Err(e) => eprintln!("warning: cannot watch {}: {e}", nice(&fresh)),
        }
    }
}

/// Strip Windows' verbatim `\\?\` prefix for readable output.
fn nice(p: &Path) -> String {
    let s = p.display().to_string();
    s.strip_prefix(r"\\?\").unwrap_or(&s).to_string()
}
