use clap::Parser;
use jsdata::version::VersionSpec;
use std::path::PathBuf;
use std::process::ExitCode;

/// Extract JS declarations into a generated wiki-data ES module.
//
// `disable_version_flag`: `--version` here means "extract as of this version",
// not "print the tool's version". That's the whole point of the flag for a
// vertion `run` line, and clap won't let both exist.
#[derive(Parser)]
#[command(about, disable_version_flag = true)]
struct Cli {
    /// Config file. Defaults to `jsdata.cfg` in the current directory, then
    /// the nearest one in a parent directory — so `jsdata` can be run from a
    /// vertion build folder and still find the project's config.
    config: Option<PathBuf>,

    /// Watch config + all referenced sources and rebuild on change
    #[arg(long, conflicts_with = "check")]
    watch: bool,

    /// Validate only; do not write output
    #[arg(long)]
    check: bool,

    /// Build one project once (the one-shot counterpart of --watch). Same
    /// config as the default build (`var`/`let`/`langs`).
    #[arg(long, conflicts_with = "watch")]
    pull: bool,

    /// Merge mode: aggregate several projects' generated wikiData modules
    /// and/or lang bundles into one namespaced module. Config is
    /// `vars NAME <-- PROJECT --> _wikiData.js` and `langs <-- TEXTS <-- languages.json`.
    #[arg(long = "pull-all", conflicts_with = "watch")]
    pull_all: bool,

    /// Extract as of a game version, filtering Vertion `//version` markers.
    /// `SPEC` is `x.y` or a quoted range `"x.y x1.y1"`. Overrides the config's
    /// `version`; a `--- ` pin overrides this. Per-project versions belong on
    /// the merge config's `vars ... --- <spec>` lines.
    /// From a vertion build: `--version %VERTION_VERSION%`.
    #[arg(long = "version", value_name = "SPEC")]
    version: Option<String>,

    /// Only keep marker blocks carrying one of these tags, exactly as Vertion's
    /// `--tag` does: tags are opt-in, so with no `--tag` at all every *tagged*
    /// block is skipped, while untagged blocks always survive. Pass `*` to admit
    /// every tag. Repeatable, and comma-separated values are split.
    /// Overrides the config's `versionTags`; a `--- <spec> [tags]` pin overrides
    /// this. From a vertion build: `--tag %VERTION_TAGS%`.
    #[arg(short = 't', long = "tag", value_name = "TAG")]
    tag: Vec<String>,

    /// Print what the config yields instead of writing it: every entry, its
    /// value shape, and — for a `const` declared more than once behind version
    /// markers — each version found and which one this run selects.
    #[arg(long, conflicts_with_all = ["watch", "check"])]
    structure: bool,
}

const DEFAULT_CONFIG: &str = "jsdata.cfg";

/// The config to read: an explicit argument, else `./jsdata.cfg`, else the
/// nearest one in an ancestor directory.
///
/// The upward search is what lets a `vertion.cfg` `run` line just say
/// `jsdata`: the command runs with `cwd` set to the build output folder
/// (`build/dev/2.5.0`), which has no config of its own. Paths inside the config
/// still resolve against the config file's own directory, so `out` keeps landing
/// in the project.
fn resolve_config(explicit: Option<PathBuf>) -> PathBuf {
    if let Some(p) = explicit {
        return p;
    }
    let here = PathBuf::from(DEFAULT_CONFIG);
    if here.is_file() {
        return here;
    }
    let found = std::env::current_dir().ok().and_then(|cwd| {
        cwd.ancestors()
            .skip(1)
            .map(|dir| dir.join(DEFAULT_CONFIG))
            .find(|c| c.is_file())
    });
    match found {
        Some(p) => {
            // A surprise parent config should be visible, not silent.
            eprintln!("using {}", p.display());
            p
        }
        None => here,
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let config = resolve_config(cli.config.clone());

    // `--tag a,b --tag c` → ["a","b","c"]. An unexpanded `%VAR%` / `$VAR` is
    // dropped with a warning: on Windows, cmd leaves `%VERTION_TAGS%` standing
    // when the variable is empty, and a literal `%VERTION_TAGS%` tag would
    // silently filter every block away.
    let mut tags: Vec<String> = Vec::new();
    for raw in cli.tag.iter().flat_map(|t| t.split(',')) {
        let t = raw.trim();
        if t.is_empty() {
            continue;
        }
        if (t.starts_with('%') && t.ends_with('%')) || t.starts_with('$') {
            eprintln!("warning: --tag {t} looks like an unexpanded variable — ignoring it");
            continue;
        }
        tags.push(t.to_string());
    }

    let opts = jsdata::Options {
        version: VersionSpec {
            spec: cli.version.clone(),
            tags: (!cli.tag.is_empty()).then_some(tags),
        },
    };

    if cli.structure {
        return match jsdata::structure::report(&config, &opts, cli.pull_all) {
            Ok(text) => {
                print!("{text}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        };
    }

    if cli.watch {
        return match jsdata::watch::watch_with(&config, &opts) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        };
    }

    // --pull is the explicit one-shot build; bare invocation does the same.
    let outcome = if cli.pull_all {
        jsdata::pull_all_with(&config, !cli.check, &opts)
    } else {
        jsdata::build_with(&config, !cli.check, &opts)
    };
    for w in &outcome.warnings {
        eprintln!("warning: {w}");
    }
    for e in &outcome.errors {
        eprintln!("error: {e}");
    }
    if !outcome.errors.is_empty() {
        return ExitCode::FAILURE;
    }
    match outcome.wrote {
        Some(true) => println!("wrote {}", outcome.out.display()),
        Some(false) => println!("unchanged {}", outcome.out.display()),
        None => println!("ok: {}", outcome.out.display()), // --check
    }
    ExitCode::SUCCESS
}
