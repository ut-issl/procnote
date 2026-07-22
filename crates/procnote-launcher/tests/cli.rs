use std::process::Command;

fn launcher() -> Command {
    Command::new(env!("CARGO_BIN_EXE_procnote-launcher"))
}

#[test]
fn version_is_printed_without_starting_the_gui() {
    let output = launcher()
        .arg("--version")
        .output()
        .expect("run launcher --version");

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).expect("version output is UTF-8"),
        format!("procnote {}\n", env!("CARGO_PKG_VERSION"))
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn help_is_printed_without_starting_the_gui() {
    let output = launcher()
        .arg("--help")
        .output()
        .expect("run launcher --help");
    let stdout = String::from_utf8(output.stdout).expect("help output is UTF-8");

    assert!(output.status.success());
    assert!(stdout.contains("Usage: procnote [WORKSPACE]"));
    assert!(stdout.contains("--version"));
    assert!(output.stderr.is_empty());
}

#[test]
fn invalid_options_fail_in_the_foreground() {
    let output = launcher()
        .arg("--not-a-procnote-option")
        .output()
        .expect("run launcher with an invalid option");
    let stderr = String::from_utf8(output.stderr).expect("error output is UTF-8");

    assert_eq!(output.status.code(), Some(2));
    assert!(stderr.contains("unexpected argument '--not-a-procnote-option'"));
    assert!(output.stdout.is_empty());
}

#[cfg(unix)]
#[test]
fn packaged_launcher_detaches_gui_and_preserves_process_context() {
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::thread;
    use std::time::Duration;

    let temporary = tempfile::tempdir().expect("create package fixture");
    let (packaged_launcher, gui) = package_paths(temporary.path());
    fs::create_dir_all(
        packaged_launcher
            .parent()
            .expect("packaged launcher has a parent"),
    )
    .expect("create launcher directory");
    fs::create_dir_all(gui.parent().expect("fake GUI has a parent")).expect("create GUI directory");
    fs::copy(env!("CARGO_BIN_EXE_procnote-launcher"), &packaged_launcher)
        .expect("copy launcher into package fixture");
    fs::set_permissions(&packaged_launcher, fs::Permissions::from_mode(0o755))
        .expect("make packaged launcher executable");

    fs::write(
        &gui,
        "#!/bin/sh\n{ printf '%s\\n%s\\n' \"$1\" \"$2\"; pwd -P; printf '%s\\n' \"$PROCNOTE_LAUNCH_TEST_ENV\"; } > \"$PROCNOTE_LAUNCH_TEST_OUTPUT.tmp\"\nmv \"$PROCNOTE_LAUNCH_TEST_OUTPUT.tmp\" \"$PROCNOTE_LAUNCH_TEST_OUTPUT\"\n",
    )
    .expect("write fake GUI");
    fs::set_permissions(&gui, fs::Permissions::from_mode(0o755)).expect("make fake GUI executable");

    let link_directory = temporary.path().join("PATH directory with spaces");
    fs::create_dir(&link_directory).expect("create symlink directory");
    let launcher_link = link_directory.join("procnote");
    symlink(&packaged_launcher, &launcher_link).expect("symlink packaged launcher");

    let workspace = temporary.path().join("workspace with spaces");
    fs::create_dir(&workspace).expect("create workspace");
    let gui_output = temporary.path().join("gui-output");
    let launcher_output = Command::new(&launcher_link)
        .arg(".")
        .current_dir(&workspace)
        .env("PROCNOTE_LAUNCH_TEST_ENV", "environment preserved")
        .env("PROCNOTE_LAUNCH_TEST_OUTPUT", &gui_output)
        .output()
        .expect("run packaged launcher");

    assert!(launcher_output.status.success());
    assert!(launcher_output.stdout.is_empty());
    assert!(launcher_output.stderr.is_empty());
    assert!(
        (0..100).any(|_| {
            if gui_output.is_file() {
                true
            } else {
                thread::sleep(Duration::from_millis(20));
                false
            }
        }),
        "detached GUI did not write its output"
    );

    let expected = format!(
        "--\n.\n{}\nenvironment preserved\n",
        workspace
            .canonicalize()
            .expect("canonicalize workspace")
            .display()
    );
    assert_eq!(
        fs::read_to_string(gui_output).expect("read detached GUI output"),
        expected
    );
}

#[cfg(target_os = "macos")]
fn package_paths(root: &std::path::Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let contents = root
        .join("Application with spaces")
        .join("procnote.app")
        .join("Contents");
    (
        contents.join("Resources").join("bin").join("procnote"),
        contents.join("MacOS").join("procnote"),
    )
}

#[cfg(target_os = "linux")]
fn package_paths(root: &std::path::Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let bin = root.join("package with spaces").join("bin");
    (bin.join("procnote"), bin.join("procnote-gui"))
}
