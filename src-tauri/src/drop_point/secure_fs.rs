use std::io::Write;
use std::path::Path;

pub fn ensure_private_directory(path: &Path) -> Result<(), String> {
    let mut missing = Vec::new();
    let mut current = path;
    loop {
        match std::fs::symlink_metadata(current) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    return Err("receiver-controlled path is not a directory".to_string());
                }
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                missing.push(current.to_path_buf());
                current = current.parent().ok_or_else(|| {
                    "receiver-controlled directory has no existing parent".to_string()
                })?;
            }
            Err(error) => return Err(error.to_string()),
        }
    }

    for directory in missing.into_iter().rev() {
        create_private_directory(&directory)?;
        let parent = directory
            .parent()
            .ok_or_else(|| "receiver-controlled directory has no parent".to_string())?;
        sync_dir(parent).map_err(|error| error.to_string())?;
    }
    set_private_directory_permissions(path)?;
    verify_private_directory(path)
}

pub fn create_private_directory(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(path)
            .map_err(|error| error.to_string())?;
    }
    #[cfg(not(unix))]
    std::fs::create_dir(path).map_err(|error| error.to_string())?;

    set_private_directory_permissions(path)
}

pub fn write_private_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = open_private_file_new(path)?;
    let result = file
        .write_all(bytes)
        .and_then(|()| file.flush())
        .and_then(|()| file.sync_all());
    drop(file);
    result.map_err(|error| error.to_string())
}

pub fn atomic_write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "private state path has no parent directory".to_string())?;
    ensure_private_directory(parent)?;
    let temporary = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("state"),
        uuid::Uuid::new_v4()
    ));

    let write_result = write_private_file(&temporary, bytes)
        .and_then(|()| atomic_replace(&temporary, path))
        .and_then(|()| set_private_file_permissions(path))
        .and_then(|()| sync_dir(parent).map_err(|error| error.to_string()));
    if write_result.is_err() {
        match std::fs::remove_file(&temporary) {
            Ok(()) => {
                let _ = sync_dir(parent);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => {}
        }
    }
    write_result
}

pub fn verify_private_directory(path: &Path) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("receiver-controlled bundle path is not a directory".to_string());
    }
    verify_owner_only(&metadata, "receiver-controlled directory")
}

pub fn verify_private_regular_file(path: &Path) -> Result<std::fs::Metadata, String> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("receiver-controlled bundle entry is not a regular file".to_string());
    }
    verify_owner_only(&metadata, "receiver-controlled file")?;
    Ok(metadata)
}

fn open_private_file_new(path: &Path) -> Result<std::fs::File, String> {
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path).map_err(|error| error.to_string())?;
    set_private_file_permissions(path)?;
    Ok(file)
}

#[cfg(unix)]
fn set_private_directory_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .map_err(|error| error.to_string())
}

#[cfg(not(unix))]
fn set_private_directory_permissions(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(unix)]
fn set_private_file_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|error| error.to_string())
}

#[cfg(not(unix))]
fn set_private_file_permissions(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(unix)]
#[expect(
    clippy::verbose_bit_mask,
    reason = "permission masks are clearer in conventional octal notation"
)]
fn verify_owner_only(metadata: &std::fs::Metadata, label: &str) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    if metadata.permissions().mode() & 0o077 == 0 {
        Ok(())
    } else {
        Err(format!("{label} is not owner-only"))
    }
}

#[cfg(not(unix))]
fn verify_owner_only(_metadata: &std::fs::Metadata, _label: &str) -> Result<(), String> {
    Ok(())
}

#[cfg(not(windows))]
fn atomic_replace(source: &Path, destination: &Path) -> Result<(), String> {
    std::fs::rename(source, destination).map_err(|error| error.to_string())
}

#[cfg(windows)]
fn atomic_replace(source: &Path, destination: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    let source_wide = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination_wide = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    // SAFETY: both paths are encoded as valid, NUL-terminated UTF-16 buffers
    // and remain alive for the duration of the call.
    let result = unsafe {
        MoveFileExW(
            source_wide.as_ptr(),
            destination_wide.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(std::io::Error::last_os_error().to_string())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
pub fn sync_dir(path: &Path) -> Result<(), std::io::Error> {
    std::fs::File::open(path)?.sync_all()
}

#[cfg(windows)]
pub fn sync_dir(path: &Path) -> Result<(), std::io::Error> {
    use std::os::windows::fs::OpenOptionsExt;

    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;

    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)?
        .sync_all()
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "unwrap is acceptable in tests")]
mod tests {
    use super::*;

    #[test]
    fn atomically_writes_owner_only_state() {
        let temporary = tempfile::tempdir().unwrap();
        let directory = temporary.path().join("state");
        let path = directory.join("session.json");
        atomic_write_private(&path, b"first").unwrap();
        atomic_write_private(&path, b"second").unwrap();
        assert_eq!(std::fs::read(path).unwrap(), b"second");
        verify_private_directory(&directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_symlink_at_the_controlled_directory_boundary() {
        use std::os::unix::fs::symlink;

        let temporary = tempfile::tempdir().unwrap();
        let target = temporary.path().join("target");
        std::fs::create_dir(&target).unwrap();
        let controlled = temporary.path().join("controlled");
        symlink(target, &controlled).unwrap();
        assert!(ensure_private_directory(&controlled).is_err());
    }
}
