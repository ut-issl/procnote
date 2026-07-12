use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::drop_point::manifest::{
    RecoveredFile, filename_collision_key, sanitize_mime_type, validate_filename,
};
use crate::drop_point::secure_fs::{
    create_private_directory, ensure_private_directory, sync_dir, verify_private_directory,
    verify_private_regular_file, write_private_file,
};

const RECEIPT_NAME: &str = ".droppoint-receipt.json";
const RECEIPT_VERSION: u32 = 1;
const IDENTITY_DOMAIN: &[u8] = b"DropPoint installed bundle v1\0";
const MAX_RECEIPT_FILES: usize = 1000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledBundle {
    pub path: PathBuf,
    pub identity: String,
    pub files: Vec<InstalledFile>,
    pub already_installed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledFile {
    pub filename: String,
    pub content_type: String,
    pub size: u64,
    pub sha256: String,
    pub relative_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct BundleReceipt {
    receipt_version: u32,
    drop_point_id: String,
    bundle_sha256: String,
    files: Vec<ReceiptFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ReceiptFile {
    name: String,
    mime_type: String,
    size: u64,
    sha256: String,
}

#[derive(Debug, thiserror::Error)]
pub enum BundleStorageError {
    #[error("invalid drop point ID for bundle installation")]
    InvalidDropPointId,
    #[error("invalid encrypted-bundle identity")]
    InvalidIdentity,
    #[error("encrypted bundle is too large to identify on this platform")]
    IdentityLength,
    #[error("recovered bundle is invalid: {0}")]
    InvalidBundle(String),
    #[error("bundle receipt is invalid: {0}")]
    InvalidReceipt(String),
    #[error("a different or damaged bundle already occupies the destination")]
    InstallationConflict,
    #[error("bundle filesystem operation failed: {0}")]
    Filesystem(String),
    #[error("bundle receipt JSON is invalid")]
    ReceiptJson,
}

pub fn encrypted_bundle_identity(
    envelope_json: &[u8],
    encrypted_payload: &[u8],
) -> Result<String, BundleStorageError> {
    let envelope_length =
        u64::try_from(envelope_json.len()).map_err(|_| BundleStorageError::IdentityLength)?;
    let payload_length =
        u64::try_from(encrypted_payload.len()).map_err(|_| BundleStorageError::IdentityLength)?;
    let mut digest = Sha256::new();
    digest.update(IDENTITY_DOMAIN);
    digest.update(envelope_length.to_be_bytes());
    digest.update(envelope_json);
    digest.update(payload_length.to_be_bytes());
    digest.update(encrypted_payload);
    Ok(hex_encode(digest.finalize().as_ref()))
}

pub fn install_bundle(
    execution_dir: &Path,
    drop_point_id: &str,
    identity: &str,
    recovered_files: &[RecoveredFile],
) -> Result<InstalledBundle, BundleStorageError> {
    validate_drop_point_id(drop_point_id)?;
    validate_identity(identity)?;
    let expected_receipt = build_receipt(drop_point_id, identity, recovered_files)?;

    verify_execution_directory(execution_dir)?;
    let attachments_dir = execution_dir.join("attachments");
    ensure_private_directory(&attachments_dir).map_err(BundleStorageError::Filesystem)?;
    let final_name = format!("bundle-{drop_point_id}");
    let _install_lock = acquire_install_lock(&attachments_dir, &final_name)?;
    cleanup_stale_staging_directories(&attachments_dir, &final_name)?;
    let final_path = attachments_dir.join(&final_name);

    match std::fs::symlink_metadata(&final_path) {
        Ok(_) => {
            verify_receipt_and_files(&final_path, &expected_receipt)?;
            sync_dir(&attachments_dir)
                .map_err(|error| BundleStorageError::Filesystem(error.to_string()))?;
            return receipt_to_install(&final_path, expected_receipt, true);
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(BundleStorageError::Filesystem(error.to_string())),
    }

    let staging_path = attachments_dir.join(format!(".{final_name}.{}.tmp", uuid::Uuid::new_v4()));
    create_private_directory(&staging_path).map_err(BundleStorageError::Filesystem)?;
    sync_dir(&attachments_dir)
        .map_err(|error| BundleStorageError::Filesystem(error.to_string()))?;

    let install_result = stage_bundle(&staging_path, &expected_receipt, recovered_files)
        .and_then(|()| publish_directory_without_replacement(&staging_path, &final_path))
        .and_then(|published| {
            if published {
                sync_dir(&attachments_dir)
                    .map_err(|error| BundleStorageError::Filesystem(error.to_string()))?;
                Ok(false)
            } else {
                verify_receipt_and_files(&final_path, &expected_receipt)?;
                Ok(true)
            }
        });

    if install_result.is_err() || staging_path.exists() {
        if let Err(error) = std::fs::remove_dir_all(&staging_path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            log::warn!("failed to remove incomplete DropPoint bundle staging directory");
        }
        let _ = sync_dir(&attachments_dir);
    }

    let already_installed = install_result?;
    receipt_to_install(&final_path, expected_receipt, already_installed)
}

pub fn verify_installed_bundle(
    path: &Path,
    drop_point_id: &str,
    identity: &str,
) -> Result<InstalledBundle, BundleStorageError> {
    validate_drop_point_id(drop_point_id)?;
    validate_identity(identity)?;
    let parent = path
        .parent()
        .ok_or(BundleStorageError::InstallationConflict)?;
    verify_private_directory(parent).map_err(|_| BundleStorageError::InstallationConflict)?;
    let receipt = load_receipt(path)?;
    if receipt.drop_point_id != drop_point_id || receipt.bundle_sha256 != identity {
        return Err(BundleStorageError::InstallationConflict);
    }
    verify_receipt_and_files(path, &receipt)?;
    receipt_to_install(path, receipt, true)
}

fn acquire_install_lock(
    attachments_dir: &Path,
    final_name: &str,
) -> Result<std::fs::File, BundleStorageError> {
    let lock_path = attachments_dir.join(format!(".{final_name}.install.lock"));
    let file = match std::fs::symlink_metadata(&lock_path) {
        Ok(_) => {
            verify_private_regular_file(&lock_path)
                .map_err(|_| BundleStorageError::InstallationConflict)?;
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&lock_path)
                .map_err(|error| BundleStorageError::Filesystem(error.to_string()))?
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut options = std::fs::OpenOptions::new();
            options.create_new(true).read(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let file = options
                .open(&lock_path)
                .map_err(|error| BundleStorageError::Filesystem(error.to_string()))?;
            file.sync_all()
                .map_err(|error| BundleStorageError::Filesystem(error.to_string()))?;
            sync_dir(attachments_dir)
                .map_err(|error| BundleStorageError::Filesystem(error.to_string()))?;
            file
        }
        Err(error) => return Err(BundleStorageError::Filesystem(error.to_string())),
    };
    file.try_lock_exclusive()
        .map_err(|error| BundleStorageError::Filesystem(error.to_string()))?;
    Ok(file)
}

#[expect(
    clippy::case_sensitive_file_extension_comparisons,
    reason = "only the installer's canonical lowercase .tmp names may be deleted"
)]
fn cleanup_stale_staging_directories(
    attachments_dir: &Path,
    final_name: &str,
) -> Result<(), BundleStorageError> {
    let prefix = format!(".{final_name}.");
    let entries = std::fs::read_dir(attachments_dir)
        .map_err(|error| BundleStorageError::Filesystem(error.to_string()))?
        .map(|entry| entry.map_err(|error| BundleStorageError::Filesystem(error.to_string())))
        .collect::<Result<Vec<_>, BundleStorageError>>()?;
    let candidates = entries
        .into_iter()
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with(&prefix) && name.ends_with(".tmp"))
        })
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return Ok(());
    }
    for candidate in candidates {
        let metadata = std::fs::symlink_metadata(&candidate)
            .map_err(|error| BundleStorageError::Filesystem(error.to_string()))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(BundleStorageError::InstallationConflict);
        }
        std::fs::remove_dir_all(candidate)
            .map_err(|error| BundleStorageError::Filesystem(error.to_string()))?;
    }
    sync_dir(attachments_dir).map_err(|error| BundleStorageError::Filesystem(error.to_string()))
}

fn verify_execution_directory(execution_dir: &Path) -> Result<(), BundleStorageError> {
    let metadata = std::fs::symlink_metadata(execution_dir)
        .map_err(|error| BundleStorageError::Filesystem(error.to_string()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(BundleStorageError::Filesystem(
            "execution destination is not a directory".to_string(),
        ));
    }
    Ok(())
}

fn build_receipt(
    drop_point_id: &str,
    identity: &str,
    recovered_files: &[RecoveredFile],
) -> Result<BundleReceipt, BundleStorageError> {
    if !(1..=MAX_RECEIPT_FILES).contains(&recovered_files.len()) {
        return Err(BundleStorageError::InvalidBundle(
            "bundle must contain between 1 and 1000 files".to_string(),
        ));
    }
    let mut comparison_keys = HashSet::with_capacity(recovered_files.len());
    let files = recovered_files
        .iter()
        .map(|recovered| {
            validate_filename(&recovered.filename).map_err(BundleStorageError::InvalidBundle)?;
            if !comparison_keys.insert(filename_collision_key(&recovered.filename)) {
                return Err(BundleStorageError::InvalidBundle(
                    "bundle contains colliding filenames".to_string(),
                ));
            }
            let canonical_mime = sanitize_mime_type(&recovered.content_type)
                .map_err(BundleStorageError::InvalidBundle)?;
            if canonical_mime != recovered.content_type {
                return Err(BundleStorageError::InvalidBundle(
                    "bundle contains a noncanonical MIME type".to_string(),
                ));
            }
            let size = u64::try_from(recovered.data.len()).map_err(|_| {
                BundleStorageError::InvalidBundle(
                    "recovered file length cannot be represented".to_string(),
                )
            })?;
            Ok(ReceiptFile {
                name: recovered.filename.clone(),
                mime_type: recovered.content_type.clone(),
                size,
                sha256: hex_encode(Sha256::digest(&recovered.data).as_ref()),
            })
        })
        .collect::<Result<Vec<_>, BundleStorageError>>()?;

    Ok(BundleReceipt {
        receipt_version: RECEIPT_VERSION,
        drop_point_id: drop_point_id.to_string(),
        bundle_sha256: identity.to_string(),
        files,
    })
}

fn stage_bundle(
    staging_path: &Path,
    receipt: &BundleReceipt,
    recovered_files: &[RecoveredFile],
) -> Result<(), BundleStorageError> {
    recovered_files.iter().try_for_each(|recovered| {
        write_private_file(&staging_path.join(&recovered.filename), &recovered.data)
            .map_err(BundleStorageError::Filesystem)
    })?;
    let mut receipt_bytes =
        serde_json::to_vec_pretty(receipt).map_err(|_| BundleStorageError::ReceiptJson)?;
    receipt_bytes.push(b'\n');
    write_private_file(&staging_path.join(RECEIPT_NAME), &receipt_bytes)
        .map_err(BundleStorageError::Filesystem)?;
    sync_dir(staging_path).map_err(|error| BundleStorageError::Filesystem(error.to_string()))
}

fn load_receipt(path: &Path) -> Result<BundleReceipt, BundleStorageError> {
    verify_private_directory(path).map_err(|_| BundleStorageError::InstallationConflict)?;
    let receipt_path = path.join(RECEIPT_NAME);
    verify_private_regular_file(&receipt_path)
        .map_err(|_| BundleStorageError::InstallationConflict)?;
    let bytes = std::fs::read(receipt_path)
        .map_err(|error| BundleStorageError::Filesystem(error.to_string()))?;
    let receipt: BundleReceipt =
        serde_json::from_slice(&bytes).map_err(|_| BundleStorageError::ReceiptJson)?;
    validate_receipt(&receipt)?;
    Ok(receipt)
}

fn validate_receipt(receipt: &BundleReceipt) -> Result<(), BundleStorageError> {
    if receipt.receipt_version != RECEIPT_VERSION {
        return Err(BundleStorageError::InvalidReceipt(
            "unsupported receipt version".to_string(),
        ));
    }
    validate_drop_point_id(&receipt.drop_point_id)?;
    validate_identity(&receipt.bundle_sha256)?;
    if !(1..=MAX_RECEIPT_FILES).contains(&receipt.files.len()) {
        return Err(BundleStorageError::InvalidReceipt(
            "invalid receipt file count".to_string(),
        ));
    }

    let mut comparison_keys = HashSet::with_capacity(receipt.files.len());
    for entry in &receipt.files {
        validate_filename(&entry.name).map_err(BundleStorageError::InvalidReceipt)?;
        if !comparison_keys.insert(filename_collision_key(&entry.name)) {
            return Err(BundleStorageError::InvalidReceipt(
                "receipt contains colliding filenames".to_string(),
            ));
        }
        let canonical_mime =
            sanitize_mime_type(&entry.mime_type).map_err(BundleStorageError::InvalidReceipt)?;
        if canonical_mime != entry.mime_type || !is_lower_hex_sha256(&entry.sha256) {
            return Err(BundleStorageError::InvalidReceipt(
                "receipt contains invalid file metadata".to_string(),
            ));
        }
    }
    Ok(())
}

fn verify_receipt_and_files(
    path: &Path,
    expected_receipt: &BundleReceipt,
) -> Result<(), BundleStorageError> {
    let actual_receipt = load_receipt(path).map_err(|error| match error {
        BundleStorageError::Filesystem(_) => error,
        _ => BundleStorageError::InstallationConflict,
    })?;
    if &actual_receipt != expected_receipt {
        return Err(BundleStorageError::InstallationConflict);
    }

    let expected_names = actual_receipt
        .files
        .iter()
        .map(|entry| entry.name.clone())
        .chain(std::iter::once(RECEIPT_NAME.to_string()))
        .collect::<HashSet<_>>();
    let actual_names = std::fs::read_dir(path)
        .map_err(|error| BundleStorageError::Filesystem(error.to_string()))?
        .map(|entry| {
            entry
                .map_err(|error| BundleStorageError::Filesystem(error.to_string()))?
                .file_name()
                .into_string()
                .map_err(|_| BundleStorageError::InstallationConflict)
        })
        .collect::<Result<HashSet<_>, BundleStorageError>>()?;
    if actual_names != expected_names {
        return Err(BundleStorageError::InstallationConflict);
    }

    for entry in &actual_receipt.files {
        let file_path = path.join(&entry.name);
        let metadata = verify_private_regular_file(&file_path)
            .map_err(|_| BundleStorageError::InstallationConflict)?;
        if metadata.len() != entry.size || hash_file(&file_path)? != entry.sha256 {
            return Err(BundleStorageError::InstallationConflict);
        }
    }
    Ok(())
}

fn receipt_to_install(
    path: &Path,
    receipt: BundleReceipt,
    already_installed: bool,
) -> Result<InstalledBundle, BundleStorageError> {
    let canonical_path = path
        .canonicalize()
        .map_err(|error| BundleStorageError::Filesystem(error.to_string()))?;
    let bundle_directory = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            BundleStorageError::InvalidReceipt("bundle path has no UTF-8 name".to_string())
        })?;
    let files = receipt
        .files
        .into_iter()
        .map(|entry| InstalledFile {
            relative_path: format!("attachments/{bundle_directory}/{}", entry.name),
            filename: entry.name,
            content_type: entry.mime_type,
            size: entry.size,
            sha256: entry.sha256,
        })
        .collect();
    Ok(InstalledBundle {
        path: canonical_path,
        identity: receipt.bundle_sha256,
        files,
        already_installed,
    })
}

fn hash_file(path: &Path) -> Result<String, BundleStorageError> {
    let mut file = std::fs::File::open(path)
        .map_err(|error| BundleStorageError::Filesystem(error.to_string()))?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| BundleStorageError::Filesystem(error.to_string()))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(hex_encode(digest.finalize().as_ref()))
}

fn publish_directory_without_replacement(
    source: &Path,
    destination: &Path,
) -> Result<bool, BundleStorageError> {
    match rename_without_replacement(source, destination) {
        Ok(()) => Ok(true),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::AlreadyExists | std::io::ErrorKind::DirectoryNotEmpty
            ) || std::fs::symlink_metadata(destination).is_ok() =>
        {
            Ok(false)
        }
        Err(error) => Err(BundleStorageError::Filesystem(error.to_string())),
    }
}

#[cfg(any(
    target_os = "android",
    target_os = "ios",
    target_os = "linux",
    target_os = "macos"
))]
fn rename_without_replacement(source: &Path, destination: &Path) -> std::io::Result<()> {
    rustix::fs::renameat_with(
        rustix::fs::CWD,
        source,
        rustix::fs::CWD,
        destination,
        rustix::fs::RenameFlags::NOREPLACE,
    )
    .map_err(std::io::Error::from)
}

#[cfg(not(any(
    target_os = "android",
    target_os = "ios",
    target_os = "linux",
    target_os = "macos"
)))]
fn rename_without_replacement(source: &Path, destination: &Path) -> std::io::Result<()> {
    if std::fs::symlink_metadata(destination).is_ok() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "bundle destination already exists",
        ));
    }
    std::fs::rename(source, destination)
}

fn validate_drop_point_id(value: &str) -> Result<(), BundleStorageError> {
    if value
        .strip_prefix("dp_")
        .is_some_and(|suffix| !suffix.is_empty() && suffix.bytes().all(is_capability_byte))
    {
        Ok(())
    } else {
        Err(BundleStorageError::InvalidDropPointId)
    }
}

fn validate_identity(value: &str) -> Result<(), BundleStorageError> {
    if is_lower_hex_sha256(value) {
        Ok(())
    } else {
        Err(BundleStorageError::InvalidIdentity)
    }
}

fn is_lower_hex_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

const fn is_capability_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().fold(
        String::with_capacity(bytes.len() * 2),
        |mut output, byte| {
            use std::fmt::Write as _;
            write!(output, "{byte:02x}").expect("writing to a String cannot fail");
            output
        },
    )
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "unwrap is acceptable in tests")]
mod tests {
    use super::*;

    fn recovered(data: &[u8]) -> Vec<RecoveredFile> {
        vec![RecoveredFile {
            filename: "scan.txt".to_string(),
            content_type: "text/plain".to_string(),
            data: data.to_vec(),
        }]
    }

    #[test]
    fn identity_binds_exact_envelope_and_payload() {
        let identity = encrypted_bundle_identity(b"envelope", b"payload").unwrap();
        assert_eq!(identity.len(), 64);
        assert_ne!(
            identity,
            encrypted_bundle_identity(b"envelope2", b"payload").unwrap()
        );
        assert_ne!(
            identity,
            encrypted_bundle_identity(b"envelope", b"payload2").unwrap()
        );
    }

    #[test]
    fn installs_and_idempotently_verifies_an_owner_only_bundle() {
        let temporary = tempfile::tempdir().unwrap();
        let execution_dir = temporary.path().join("execution");
        std::fs::create_dir(&execution_dir).unwrap();
        let identity = encrypted_bundle_identity(b"envelope", b"payload").unwrap();

        let first = install_bundle(
            &execution_dir,
            "dp_example",
            &identity,
            &recovered(b"hello"),
        )
        .unwrap();
        assert!(!first.already_installed);
        assert_eq!(
            first.files[0].relative_path,
            "attachments/bundle-dp_example/scan.txt"
        );

        let second = install_bundle(
            &execution_dir,
            "dp_example",
            &identity,
            &recovered(b"hello"),
        )
        .unwrap();
        assert!(second.already_installed);
        assert_eq!(first.path, second.path);
        verify_installed_bundle(&first.path, "dp_example", &identity).unwrap();
    }

    #[test]
    fn removes_crash_leftover_staging_before_retrying_installation() {
        let temporary = tempfile::tempdir().unwrap();
        let execution_dir = temporary.path().join("execution");
        let attachments_dir = execution_dir.join("attachments");
        std::fs::create_dir_all(&attachments_dir).unwrap();
        let stale = attachments_dir.join(".bundle-dp_example.crashed.tmp");
        std::fs::create_dir(&stale).unwrap();
        std::fs::write(stale.join("partial.txt"), b"partial plaintext").unwrap();
        let identity = encrypted_bundle_identity(b"envelope", b"payload").unwrap();

        install_bundle(
            &execution_dir,
            "dp_example",
            &identity,
            &recovered(b"complete"),
        )
        .unwrap();
        assert!(!stale.exists());
    }

    #[test]
    fn never_overwrites_or_merges_a_conflicting_bundle() {
        let temporary = tempfile::tempdir().unwrap();
        let execution_dir = temporary.path().join("execution");
        std::fs::create_dir(&execution_dir).unwrap();
        let first_identity = encrypted_bundle_identity(b"one", b"payload").unwrap();
        let first = install_bundle(
            &execution_dir,
            "dp_example",
            &first_identity,
            &recovered(b"first"),
        )
        .unwrap();
        let second_identity = encrypted_bundle_identity(b"two", b"payload").unwrap();
        assert!(matches!(
            install_bundle(
                &execution_dir,
                "dp_example",
                &second_identity,
                &recovered(b"second")
            ),
            Err(BundleStorageError::InstallationConflict)
        ));
        assert_eq!(
            std::fs::read(first.path.join("scan.txt")).unwrap(),
            b"first"
        );
    }

    #[test]
    fn rejects_extra_and_changed_entries() {
        let temporary = tempfile::tempdir().unwrap();
        let execution_dir = temporary.path().join("execution");
        std::fs::create_dir(&execution_dir).unwrap();
        let identity = encrypted_bundle_identity(b"one", b"payload").unwrap();
        let installed = install_bundle(
            &execution_dir,
            "dp_example",
            &identity,
            &recovered(b"first"),
        )
        .unwrap();

        std::fs::write(installed.path.join("extra"), b"").unwrap();
        assert!(verify_installed_bundle(&installed.path, "dp_example", &identity).is_err());
        std::fs::remove_file(installed.path.join("extra")).unwrap();

        std::fs::write(installed.path.join("scan.txt"), b"changed").unwrap();
        assert!(verify_installed_bundle(&installed.path, "dp_example", &identity).is_err());
    }

    #[test]
    fn rejects_missing_and_non_regular_installed_files() {
        let temporary = tempfile::tempdir().unwrap();
        let execution_dir = temporary.path().join("execution");
        std::fs::create_dir(&execution_dir).unwrap();
        let identity = encrypted_bundle_identity(b"one", b"payload").unwrap();
        let installed = install_bundle(
            &execution_dir,
            "dp_example",
            &identity,
            &recovered(b"first"),
        )
        .unwrap();
        let file_path = installed.path.join("scan.txt");

        std::fs::remove_file(&file_path).unwrap();
        assert!(verify_installed_bundle(&installed.path, "dp_example", &identity).is_err());
        std::fs::create_dir(&file_path).unwrap();
        assert!(verify_installed_bundle(&installed.path, "dp_example", &identity).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_non_private_installed_files_and_receipts() {
        use std::os::unix::fs::PermissionsExt;

        let temporary = tempfile::tempdir().unwrap();
        let execution_dir = temporary.path().join("execution");
        std::fs::create_dir(&execution_dir).unwrap();
        let identity = encrypted_bundle_identity(b"one", b"payload").unwrap();
        let installed = install_bundle(
            &execution_dir,
            "dp_example",
            &identity,
            &recovered(b"first"),
        )
        .unwrap();
        let file_path = installed.path.join("scan.txt");
        std::fs::set_permissions(&file_path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(verify_installed_bundle(&installed.path, "dp_example", &identity).is_err());

        std::fs::set_permissions(&file_path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let receipt_path = installed.path.join(RECEIPT_NAME);
        std::fs::set_permissions(&receipt_path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(verify_installed_bundle(&installed.path, "dp_example", &identity).is_err());

        std::fs::set_permissions(&receipt_path, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::set_permissions(&installed.path, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(verify_installed_bundle(&installed.path, "dp_example", &identity).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_installed_files() {
        use std::os::unix::fs::symlink;

        let temporary = tempfile::tempdir().unwrap();
        let execution_dir = temporary.path().join("execution");
        std::fs::create_dir(&execution_dir).unwrap();
        let identity = encrypted_bundle_identity(b"one", b"payload").unwrap();
        let installed = install_bundle(
            &execution_dir,
            "dp_example",
            &identity,
            &recovered(b"first"),
        )
        .unwrap();
        std::fs::remove_file(installed.path.join("scan.txt")).unwrap();
        symlink("elsewhere", installed.path.join("scan.txt")).unwrap();
        assert!(verify_installed_bundle(&installed.path, "dp_example", &identity).is_err());
    }
}
