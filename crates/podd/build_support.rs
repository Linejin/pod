//! Build-only ConPTY helpers, also exercised by the holder contract target.
use std::path::Path;

pub fn profile_dir(out: &Path) -> Option<&Path> {
    out.ancestors()
        .find(|dir| dir.file_name().is_some_and(|name| name == "build"))
        .and_then(Path::parent)
}

pub fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    let entries = std::fs::read_dir(from)?;
    std::fs::create_dir_all(to)?;
    for e in entries {
        let e = e?;
        let p = e.path();
        let name = e.file_name();
        if p.is_dir() {
            copy_tree(&p, &to.join(&name))?;
        } else if p.extension().is_some_and(|x| x == "dll" || x == "exe") {
            let dst = to.join(&name);
            let bytes = std::fs::read(&p)?;
            match std::fs::read(&dst) {
                Ok(current) if current == bytes => continue,
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    return Err(std::io::Error::new(
                        e.kind(),
                        format!("cannot inspect {}: {e}", dst.display()),
                    ));
                }
            }
            // Preserve a stale destination if it is locked by a running
            // holder. A failed replacement must be visible to the build.
            let tmp = dst.with_extension(format!("pod-copy-{}.tmp", std::process::id()));
            let replaced = std::fs::write(&tmp, bytes).and_then(|()| std::fs::rename(&tmp, &dst));
            if let Err(e) = replaced {
                let _ = std::fs::remove_file(&tmp);
                return Err(std::io::Error::new(
                    e.kind(),
                    format!("cannot update {} from {}: {e}", dst.display(), p.display()),
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::fs::OpenOptionsExt;

    fn root(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("pod-build-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(root.join("vendor")).unwrap();
        std::fs::create_dir_all(root.join("output")).unwrap();
        root
    }

    #[test]
    fn both_cargo_out_dir_layouts_locate_the_binary_profile() {
        let profile = Path::new(r"C:\fixture target\debug");
        for suffix in [r"build\podd-01234567\out", r"build\podd\01234567\out"] {
            let out = profile.join(suffix);
            assert_eq!(profile_dir(&out), Some(profile));
        }
        assert_eq!(profile_dir(Path::new(r"C:\fixture\out")), None);
    }

    #[test]
    fn equal_length_changed_bundle_is_replaced_by_its_actual_bytes() {
        let root = root("equal");
        std::fs::write(root.join("vendor/conpty.dll"), b"DLL-new").unwrap();
        std::fs::write(root.join("output/conpty.dll"), b"DLL-old").unwrap();
        copy_tree(&root.join("vendor"), &root.join("output")).unwrap();
        assert_eq!(
            std::fs::read(root.join("output/conpty.dll")).unwrap(),
            b"DLL-new"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn locked_stale_bundle_is_preserved_and_reported_then_retried() {
        let root = root("locked");
        let dst = root.join("output/conpty.dll");
        std::fs::write(root.join("vendor/conpty.dll"), b"new bundle bytes").unwrap();
        std::fs::write(&dst, b"old").unwrap();
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&dst)
            .unwrap();
        assert!(
            copy_tree(&root.join("vendor"), &root.join("output")).is_err(),
            "a locked stale payload must fail visibly"
        );
        assert_eq!(std::fs::read(&dst).unwrap(), b"old");
        drop(lock);
        copy_tree(&root.join("vendor"), &root.join("output")).unwrap();
        assert_eq!(std::fs::read(&dst).unwrap(), b"new bundle bytes");
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&dst)
            .unwrap();
        copy_tree(&root.join("vendor"), &root.join("output")).unwrap();
        drop(lock);
        std::fs::remove_dir_all(root).unwrap();
    }
}
