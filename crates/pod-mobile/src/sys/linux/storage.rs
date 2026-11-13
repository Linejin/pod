//! Linux generation storage, using the key store's euid/no-follow rules.
use super::identity::{O_NOFOLLOW, effective_uid};
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;

fn check(file: File, directory: bool) -> io::Result<File> {
    let m = file.metadata()?;
    if m.uid() != effective_uid()
        || m.permissions().mode() & 0o7777 != if directory { 0o700 } else { 0o600 }
        || m.is_dir() != directory
        || (!directory && !m.is_file())
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "generation owner or mode",
        ));
    }
    Ok(file)
}
pub(crate) fn directory(path: &Path, create: bool) -> io::Result<File> {
    for part in path.ancestors().filter(|p| !p.as_os_str().is_empty()) {
        match fs::symlink_metadata(part) {
            Ok(m) if m.file_type().is_symlink() => {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "generation symlink",
                ));
            }
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    if create && !path.try_exists()? {
        DirBuilder::new().mode(0o700).create(path)?;
    }
    check(
        OpenOptions::new()
            .read(true)
            .custom_flags(O_NOFOLLOW)
            .open(path)?,
        true,
    )
}
pub(crate) fn read(path: &Path) -> io::Result<File> {
    check(
        OpenOptions::new()
            .read(true)
            .custom_flags(O_NOFOLLOW)
            .open(path)?,
        false,
    )
}
pub(crate) fn create(path: &Path) -> io::Result<File> {
    check(
        OpenOptions::new()
            .write(true)
            .read(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(O_NOFOLLOW)
            .open(path)?,
        false,
    )
}
pub(crate) fn replace(temp: &Path, target: &Path, directory: &File) -> io::Result<()> {
    fs::rename(temp, target)?;
    directory.sync_all()
}
