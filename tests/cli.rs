//! Tests for the real binary: argument parsing, exit codes, and what lands on
//! stdout versus stderr.
//!
//! `tests/integration.rs` drives the same functionality through the library, so
//! this file deliberately does *not* re-test extraction or filtering semantics.
//! It covers only what exists at the process boundary and is invisible from a
//! library call — flag conflicts, the parent-directory config search, exit
//! status, and the difference between a warning and an error.

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use tempfile::TempDir;

fn jsdata() -> Command {
    Command::cargo_bin("jsdata").expect("the binary builds")
}

/// A minimal project: one source file, one config extracting from it.
fn project() -> TempDir {
    let d = tempfile::tempdir().unwrap();
    fs::create_dir_all(d.path().join("src")).unwrap();
    fs::write(d.path().join("src/a.js"), "export const A = { x: 1 };\n").unwrap();
    fs::write(
        d.path().join("jsdata.cfg"),
        "out out.js\nvar A = src/a.js -> const A\n",
    )
    .unwrap();
    d
}

/// A source whose second field is gated behind a `2.1` marker.
fn versioned() -> TempDir {
    let d = tempfile::tempdir().unwrap();
    fs::create_dir_all(d.path().join("src")).unwrap();
    fs::write(
        d.path().join("src/a.js"),
        "export const A = {\n  base: 1,\n  //version 2.1 *\n  next: 2,\n  //version 2.1 *\n};\n",
    )
    .unwrap();
    fs::write(
        d.path().join("jsdata.cfg"),
        "out out.js\nvar A = src/a.js -> const A\n",
    )
    .unwrap();
    d
}

#[test]
fn bare_invocation_builds_and_names_what_it_wrote() {
    let d = project();
    jsdata()
        .current_dir(d.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("wrote"))
        .stdout(predicate::str::contains("out.js"));

    let out = fs::read_to_string(d.path().join("out.js")).unwrap();
    assert!(out.contains("export const A = { x: 1 };"), "{out}");
}

#[test]
fn a_second_run_reports_unchanged_rather_than_rewriting() {
    let d = project();
    jsdata().current_dir(d.path()).assert().success();
    jsdata()
        .current_dir(d.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("unchanged"));
}

#[test]
fn pull_is_the_explicit_form_of_a_bare_run() {
    let d = project();
    jsdata()
        .current_dir(d.path())
        .arg("--pull")
        .assert()
        .success()
        .stdout(predicate::str::contains("wrote"));
    assert!(d.path().join("out.js").is_file());
}

#[test]
fn check_validates_without_writing() {
    let d = project();
    jsdata()
        .current_dir(d.path())
        .arg("--check")
        .assert()
        .success()
        .stdout(predicate::str::contains("ok:"));
    assert!(
        !d.path().join("out.js").exists(),
        "--check must not write the module"
    );
}

#[test]
fn a_missing_declaration_is_an_error_with_a_failing_exit_code() {
    let d = project();
    fs::write(
        d.path().join("jsdata.cfg"),
        "out out.js\nvar A = src/a.js -> const NOPE\n",
    )
    .unwrap();

    jsdata()
        .current_dir(d.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("error:"))
        .stderr(predicate::str::contains("NOPE"));
    assert!(
        !d.path().join("out.js").exists(),
        "an error must block the write"
    );
}

#[test]
fn a_missing_config_fails_rather_than_writing_an_empty_module() {
    let d = tempfile::tempdir().unwrap();
    jsdata()
        .current_dir(d.path())
        .arg("nonexistent.cfg")
        .assert()
        .failure()
        .stderr(predicate::str::contains("error:"));
}

#[test]
fn warnings_go_to_stderr_without_failing_the_run() {
    let d = project();
    // `let` referencing a name declared later is a TDZ error at module load,
    // which jsdata warns about rather than refusing to build.
    fs::write(
        d.path().join("jsdata.cfg"),
        "out out.js\nlet COUNT = Object.keys(A).length\nvar A = src/a.js -> const A\n",
    )
    .unwrap();

    jsdata()
        .current_dir(d.path())
        .assert()
        .success()
        .stderr(predicate::str::contains("warning:"))
        .stderr(predicate::str::contains("TDZ"));
    assert!(d.path().join("out.js").is_file(), "a warning still builds");
}

#[test]
fn structure_reports_without_writing() {
    let d = project();
    jsdata()
        .current_dir(d.path())
        .arg("--structure")
        .assert()
        .success()
        .stdout(predicate::str::contains("jsdata.cfg"))
        .stdout(predicate::str::contains('A'));
    assert!(
        !d.path().join("out.js").exists(),
        "--structure must not write"
    );
}

#[test]
fn an_explicit_config_path_is_used_verbatim() {
    let d = project();
    fs::rename(d.path().join("jsdata.cfg"), d.path().join("other.cfg")).unwrap();
    jsdata()
        .current_dir(d.path())
        .arg("other.cfg")
        .assert()
        .success()
        .stdout(predicate::str::contains("wrote"));
    assert!(d.path().join("out.js").is_file());
}

#[test]
fn the_config_is_found_in_a_parent_directory() {
    // This is what lets a vertion `run` line say plain `jsdata` even though the
    // build runs with its cwd set to the output folder.
    let d = project();
    let deep = d.path().join("build/dev/2.5.0");
    fs::create_dir_all(&deep).unwrap();

    jsdata()
        .current_dir(&deep)
        .assert()
        .success()
        // the surprise parent config is announced, not silent
        .stderr(predicate::str::contains("using"))
        .stdout(predicate::str::contains("wrote"));

    // `out` still resolves against the config's directory, not the cwd
    assert!(
        d.path().join("out.js").is_file(),
        "out must land next to the config, not in the build folder"
    );
    assert!(!deep.join("out.js").exists());
}

#[test]
fn version_flag_filters_marker_blocks() {
    let d = versioned();

    jsdata()
        .current_dir(d.path())
        .args(["--version", "2.0"])
        .assert()
        .success();
    let below = fs::read_to_string(d.path().join("out.js")).unwrap();
    assert!(below.contains("base: 1"), "{below}");
    assert!(!below.contains("next: 2"), "{below}");

    jsdata()
        .current_dir(d.path())
        .args(["--version", "2.1"])
        .assert()
        .success();
    let at = fs::read_to_string(d.path().join("out.js")).unwrap();
    assert!(at.contains("next: 2"), "{at}");
}

#[test]
fn an_invalid_version_spec_is_rejected() {
    let d = versioned();
    jsdata()
        .current_dir(d.path())
        .args(["--version", "notaversion"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("error:"));
}

#[test]
fn an_unexpanded_variable_tag_is_dropped_with_a_warning() {
    // cmd leaves `%VERTION_TAGS%` standing when the variable is empty, and a
    // literal tag by that name would silently filter every block away.
    let d = versioned();
    jsdata()
        .current_dir(d.path())
        .args(["--version", "2.1", "--tag", "%VERTION_TAGS%"])
        .assert()
        .success()
        .stderr(predicate::str::contains("unexpanded variable"));
}

#[test]
fn watch_and_check_cannot_be_combined() {
    let d = project();
    jsdata()
        .current_dir(d.path())
        .args(["--watch", "--check"])
        .assert()
        .code(2) // clap's usage-error exit code
        .stderr(predicate::str::contains("cannot be used with"));
}

#[test]
fn watch_fails_fast_on_a_config_it_cannot_resolve() {
    // Guards against the watcher blocking forever in CI: an unresolvable config
    // must error out before it starts watching anything.
    let d = tempfile::tempdir().unwrap();
    jsdata()
        .current_dir(d.path())
        .args(["--watch", "nonexistent.cfg"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("error:"));
}

#[test]
fn pull_all_merges_a_built_project_into_a_namespaced_module() {
    let d = project();
    jsdata().current_dir(d.path()).assert().success();

    fs::write(
        d.path().join("all.cfg"),
        "out all.js\nvars Demo <-- . --> out.js\n",
    )
    .unwrap();

    jsdata()
        .current_dir(d.path())
        .args(["--pull-all", "all.cfg"])
        .assert()
        .success()
        .stdout(predicate::str::contains("wrote"));

    let all = fs::read_to_string(d.path().join("all.js")).unwrap();
    assert!(all.contains("export const Demo = (() => {"), "{all}");
    assert!(all.contains("return { A };"), "{all}");
}

#[test]
fn help_lists_the_flags_and_explains_the_version_trap() {
    // `--version` here means "extract as of this version", so `--help` is the
    // only place the flag list is discoverable.
    jsdata()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("--pull-all"))
        .stdout(predicate::str::contains("--structure"))
        .stdout(predicate::str::contains("Extract as of a game version"));
}
