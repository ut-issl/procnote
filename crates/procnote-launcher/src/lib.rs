use std::env;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum LaunchError {
    #[error("could not determine the launcher executable path")]
    CurrentExecutable(#[source] io::Error),

    #[error("could not resolve launcher executable path {}", path.display())]
    ResolveLauncher {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("launcher path {} does not match the expected {layout} package layout", path.display())]
    InvalidPackageLayout { path: PathBuf, layout: &'static str },

    #[error("could not start application executable {}", path.display())]
    Spawn {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PackageLayout {
    name: &'static str,
    ancestor_levels: usize,
    gui_components: &'static [&'static str],
}

/// Starts the packaged GUI for `workspace` and returns as soon as the child has
/// been created.
///
/// The child inherits the launcher's current working directory and environment,
/// but is placed in a detached process group/session with null standard streams.
pub fn launch(workspace: &Path) -> Result<(), LaunchError> {
    let launcher = env::current_exe().map_err(LaunchError::CurrentExecutable)?;
    let launcher = launcher
        .canonicalize()
        .map_err(|source| LaunchError::ResolveLauncher {
            path: launcher,
            source,
        })?;
    let gui = packaged_gui_path(&launcher, current_package_layout())?;

    spawn_detached(&gui, workspace)
        .map(|_| ())
        .map_err(|source| LaunchError::Spawn { path: gui, source })
}

fn packaged_gui_path(launcher: &Path, layout: PackageLayout) -> Result<PathBuf, LaunchError> {
    let invalid_layout = || LaunchError::InvalidPackageLayout {
        path: launcher.to_path_buf(),
        layout: layout.name,
    };
    let package_root = (0..layout.ancestor_levels)
        .try_fold(launcher, |path, _| path.parent().ok_or_else(invalid_layout))?;

    Ok(layout
        .gui_components
        .iter()
        .fold(package_root.to_path_buf(), |path, component| {
            path.join(component)
        }))
}

fn spawn_detached(gui: &Path, workspace: &Path) -> io::Result<Child> {
    detached_command(gui, workspace).spawn()
}

fn detached_command(gui: &Path, workspace: &Path) -> Command {
    let mut command = Command::new(gui);
    command
        .arg("--")
        .arg(workspace)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    configure_detachment(&mut command);
    command
}

#[cfg(unix)]
#[expect(
    unsafe_code,
    reason = "stable CommandExt does not yet expose a safe setsid operation"
)]
fn configure_detachment(command: &mut Command) {
    use std::os::unix::process::CommandExt;

    // SAFETY: POSIX specifies setsid as async-signal-safe. The closure performs
    // only that call and captures no state, satisfying pre_exec's requirements.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
}

#[cfg(windows)]
fn configure_detachment(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::System::Threading::{CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS};

    command.creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS);
}

#[cfg(target_os = "windows")]
const fn current_package_layout() -> PackageLayout {
    PackageLayout {
        name: "Windows",
        ancestor_levels: 2,
        gui_components: &["procnote.exe"],
    }
}

#[cfg(target_os = "macos")]
const fn current_package_layout() -> PackageLayout {
    PackageLayout {
        name: "macOS",
        ancestor_levels: 3,
        gui_components: &["MacOS", "procnote"],
    }
}

#[cfg(target_os = "linux")]
const fn current_package_layout() -> PackageLayout {
    PackageLayout {
        name: "Linux",
        ancestor_levels: 1,
        gui_components: &["procnote-gui"],
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
compile_error!("procnote-launcher supports Windows, macOS, and Linux only");

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::*;

    const WINDOWS_LAYOUT: PackageLayout = PackageLayout {
        name: "Windows",
        ancestor_levels: 2,
        gui_components: &["procnote.exe"],
    };
    const MACOS_LAYOUT: PackageLayout = PackageLayout {
        name: "macOS",
        ancestor_levels: 3,
        gui_components: &["MacOS", "procnote"],
    };
    const LINUX_LAYOUT: PackageLayout = PackageLayout {
        name: "Linux",
        ancestor_levels: 1,
        gui_components: &["procnote-gui"],
    };

    #[test]
    fn resolves_windows_package_layout() {
        let launcher = Path::new("/install/procnote/bin/procnote.exe");

        assert_eq!(
            packaged_gui_path(launcher, WINDOWS_LAYOUT).expect("valid Windows package layout"),
            Path::new("/install/procnote/procnote.exe")
        );
    }

    #[test]
    fn resolves_macos_package_layout() {
        let launcher = Path::new("/Applications/procnote.app/Contents/Resources/bin/procnote");

        assert_eq!(
            packaged_gui_path(launcher, MACOS_LAYOUT).expect("valid macOS package layout"),
            Path::new("/Applications/procnote.app/Contents/MacOS/procnote")
        );
    }

    #[test]
    fn resolves_linux_package_layout() {
        let launcher = Path::new("/usr/bin/procnote");

        assert_eq!(
            packaged_gui_path(launcher, LINUX_LAYOUT).expect("valid Linux package layout"),
            Path::new("/usr/bin/procnote-gui")
        );
    }

    #[test]
    fn rejects_launcher_outside_expected_layout() {
        let error = packaged_gui_path(Path::new("procnote"), WINDOWS_LAYOUT)
            .expect_err("a relative filename has too few ancestors");

        assert!(matches!(error, LaunchError::InvalidPackageLayout { .. }));
    }

    #[test]
    fn command_preserves_workspace_as_one_os_argument() {
        let command = detached_command(
            Path::new("procnote-gui"),
            Path::new("workspace with spaces"),
        );

        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            [OsStr::new("--"), OsStr::new("workspace with spaces")]
        );
        assert_eq!(command.get_current_dir(), None);
        assert_eq!(command.get_envs().count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn detached_child_inherits_working_directory_and_receives_workspace() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let temporary = tempfile::tempdir().expect("create temporary package");
        let gui = temporary.path().join("fake gui");
        let output = temporary.path().join("fake gui.output");
        fs::write(
            &gui,
            "#!/bin/sh\nprintf '%s\\n%s\\n%s\\n' \"$1\" \"$2\" \"$PWD\" > \"$0.output\"\n",
        )
        .expect("write fake GUI");
        fs::set_permissions(&gui, fs::Permissions::from_mode(0o755))
            .expect("make fake GUI executable");

        let workspace = Path::new("workspace with spaces");
        let mut child = spawn_detached(&gui, workspace).expect("spawn fake GUI");
        assert!(child.wait().expect("wait for fake GUI").success());

        let lines = fs::read_to_string(output).expect("read fake GUI output");
        let expected = format!(
            "--\n{}\n{}\n",
            workspace.display(),
            env::current_dir()
                .expect("read current directory")
                .display()
        );
        assert_eq!(lines, expected);
    }
}
