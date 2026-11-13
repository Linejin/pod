//! Isolated generation persistence/rotation tests; no real state directory.
#![cfg(feature = "mobile-lan")]
mod common;
use common::TestStore;
use pod_mobile::{
    crypto,
    generation::{self, Generation, GenerationError, RECORD_FILE, RotationStage},
    tls::VersionPolicy,
};
use std::{
    fs, io,
    net::SocketAddr,
    path::PathBuf,
    time::{Duration, SystemTime},
};

struct Fixture {
    keys: TestStore,
    dir: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        Self {
            keys: TestStore::new("generation"),
            dir: std::env::temp_dir().join(format!(
                "PodMobileTest-generation-{}",
                crypto::spki_sha256(&crypto::random_256().unwrap())
                    .unwrap()
                    .to_hex()
            )),
        }
    }
    fn rotate(&self, port: u16) -> Generation {
        generation::rotate(
            &self.dir,
            &self.keys.store,
            SocketAddr::from(([127, 0, 0, 1], port)),
            VersionPolicy::platform_default(),
            SystemTime::now(),
        )
        .unwrap()
    }
    fn load(&self) -> Generation {
        generation::load(&self.dir, &self.keys.store).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn checksum(bytes: &mut Vec<u8>) {
    let n = bytes.len() - 32;
    bytes.truncate(n);
    bytes.extend(crypto::sha256(bytes).unwrap());
}
fn offsets(bytes: &[u8]) -> Vec<usize> {
    let mut cursor = 8;
    let mut out = Vec::new();
    for _ in 0..8 {
        out.push(cursor);
        cursor +=
            5 + u32::from_be_bytes(bytes[cursor + 1..cursor + 5].try_into().unwrap()) as usize;
    }
    out
}

#[test]
fn strict_record_and_stored_key_binding() {
    let t = Fixture::new();
    assert!(matches!(
        generation::load(&t.dir, &t.keys.store),
        Err(GenerationError::NoValidGeneration)
    ));
    let first = t.rotate(8443);
    assert_eq!(t.load(), first);
    let bytes = first.encode().unwrap();
    let off = offsets(&bytes);
    let mut cases = Vec::new();
    for (at, val) in [
        (7, b'2'),
        (off[0], 99),
        (off[1], 1),
        (off[3], 5),
        (off[1] + 4, 31),
        (off[7] + 5, 99),
    ] {
        let mut b = bytes.clone();
        b[at] = val;
        checksum(&mut b);
        cases.push(b);
    }
    let mut b = bytes.clone();
    b[off[1] + 5] ^= 1;
    checksum(&mut b);
    cases.push(b); // pin
    let mut b = bytes.clone();
    b[off[0] + 5] = 0;
    checksum(&mut b);
    cases.push(b); // DER
    let mut b = bytes.clone();
    b[off[3] + 5..off[3] + 13].fill(0);
    checksum(&mut b);
    cases.push(b); // epoch zero
    let mut b = bytes.clone();
    b.remove(off[7]);
    checksum(&mut b);
    cases.push(b); // missing
    let mut b = bytes.clone();
    let end = b.len() - 32;
    b.insert(end, 0);
    checksum(&mut b);
    cases.push(b); // trailing
    let mut b = bytes.clone();
    b[12] ^= 1;
    cases.push(b); // checksum
    for b in cases {
        assert!(Generation::decode(&b, &t.keys.store).is_err());
    }
    for len in 0..bytes.len() {
        assert!(
            Generation::decode(&bytes[..len], &t.keys.store).is_err(),
            "length {len}"
        );
    }
    let mut changed = first.clone();
    changed.not_after += 1;
    assert!(Generation::decode(&changed.encode().unwrap(), &t.keys.store).is_err());
    changed = first.clone();
    changed.certificate_der.push(0);
    assert!(Generation::decode(&changed.encode().unwrap(), &t.keys.store).is_err());
    let other = pod_mobile::identity::ServerIdentity::generate(
        &t.keys.store,
        &pod_mobile::identity::IdentityParams::new(first.endpoint.ip()),
    )
    .unwrap();
    changed = first.clone();
    changed.key_name = other.key_name().into();
    assert!(Generation::decode(&changed.encode().unwrap(), &t.keys.store).is_err());
    t.keys.store.delete(&first.key_name).unwrap();
    assert!(generation::load(&t.dir, &t.keys.store).is_err());
}

#[test]
fn rotation_endpoint_epoch_expiry_and_overflow() {
    let t = Fixture::new();
    let first = t.rotate(8443);
    let second = t.rotate(8444);
    assert_eq!((first.key_epoch, second.key_epoch), (1, 2));
    assert_ne!(first.spki_sha256, second.spki_sha256);
    assert_eq!(t.keys.store.keys().unwrap(), vec![second.key_name.clone()]);
    let threshold =
        SystemTime::UNIX_EPOCH + Duration::from_secs((second.not_after - 14 * 86400) as u64);
    assert!(!second.expiry_warning(threshold - Duration::from_secs(1)));
    assert!(second.expiry_warning(threshold));
    assert!(second.expiry_warning(threshold + Duration::from_secs(15 * 86400)));
    let third = generation::rotate(
        &t.dir,
        &t.keys.store,
        "[::1]:8444".parse().unwrap(),
        VersionPolicy::platform_default(),
        SystemTime::now(),
    )
    .unwrap();
    assert_eq!(third.key_epoch, 3);
    assert_eq!(t.load(), third);
    let mut max = third.clone();
    max.key_epoch = u64::MAX;
    fs::write(t.dir.join(RECORD_FILE), max.encode().unwrap()).unwrap();
    assert!(
        generation::rotate(
            &t.dir,
            &t.keys.store,
            third.endpoint,
            third.version_policy,
            SystemTime::now()
        )
        .is_err()
    );
    assert_eq!(t.load(), max);
}

#[test]
fn failed_rotation_never_mixes_or_falls_back() {
    for stage in [RotationStage::AfterTempWrite, RotationStage::Replace] {
        let t = Fixture::new();
        assert!(
            generation::rotate_with_hook(
                &t.dir,
                &t.keys.store,
                "127.0.0.1:8443".parse().unwrap(),
                VersionPolicy::platform_default(),
                SystemTime::now(),
                |s| if s == stage {
                    Err(io::Error::other("injected"))
                } else {
                    Ok(())
                }
            )
            .is_err()
        );
        assert!(t.keys.store.keys().unwrap().is_empty());
        assert_eq!(fs::read_dir(&t.dir).unwrap().count(), 0);
        assert!(generation::load(&t.dir, &t.keys.store).is_err());
    }
    for stage in [
        RotationStage::AfterTempWrite,
        RotationStage::Replace,
        RotationStage::OldKeyDelete,
    ] {
        let t = Fixture::new();
        let first = t.rotate(8443);
        let result = generation::rotate_with_hook(
            &t.dir,
            &t.keys.store,
            "127.0.0.1:8444".parse().unwrap(),
            first.version_policy,
            SystemTime::now(),
            |s| {
                if s == stage {
                    Err(io::Error::other("injected"))
                } else {
                    Ok(())
                }
            },
        );
        assert!(result.is_err());
        let actual = t.load();
        if stage == RotationStage::OldKeyDelete {
            assert!(matches!(result, Err(GenerationError::RetirementFailed)));
            assert_eq!(actual.key_epoch, 2);
            assert_eq!(actual.endpoint.port(), 8444);
            // Explicit recovery retires the orphan; test store also cleans on panic.
            t.keys.store.delete(&first.key_name).unwrap();
        } else {
            assert_eq!(actual, first);
        }
        assert_eq!(t.keys.store.keys().unwrap(), vec![actual.key_name]);
        assert_eq!(fs::read_dir(&t.dir).unwrap().count(), 1);
        fs::write(t.dir.join(RECORD_FILE), b"torn").unwrap();
        assert!(generation::load(&t.dir, &t.keys.store).is_err());
        assert!(
            generation::rotate(
                &t.dir,
                &t.keys.store,
                first.endpoint,
                first.version_policy,
                SystemTime::now()
            )
            .is_err()
        );
        assert_eq!(fs::read(t.dir.join(RECORD_FILE)).unwrap(), b"torn");
    }
}

#[test]
fn exclusive_writer_lock_refuses_second_rotation() {
    let t = Fixture::new();
    let old = t.rotate(8443);
    generation::rotate_with_hook(
        &t.dir,
        &t.keys.store,
        old.endpoint,
        old.version_policy,
        SystemTime::now(),
        |stage| {
            if stage == RotationStage::AfterTempWrite {
                assert!(
                    generation::rotate(
                        &t.dir,
                        &t.keys.store,
                        old.endpoint,
                        old.version_policy,
                        SystemTime::now()
                    )
                    .is_err()
                );
                assert_eq!(t.load(), old);
            }
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(t.load().key_epoch, 2);
}

#[cfg(windows)]
#[test]
fn windows_extra_system_ace_is_refused_on_file_and_directory() {
    for directory in [false, true] {
        let t = Fixture::new();
        t.rotate(8443);
        let path = if directory {
            t.dir.clone()
        } else {
            t.dir.join(RECORD_FILE)
        };
        // A SYSTEM ACE is explicitly outside the user-only policy. icacls
        // only touches this test's randomly named temporary path.
        let result = std::process::Command::new("icacls.exe")
            .arg(&path)
            .args(["/grant", "*S-1-5-18:(F)"])
            .output()
            .unwrap();
        assert!(result.status.success(), "icacls test fixture failed");
        assert!(matches!(
            generation::load(&t.dir, &t.keys.store),
            Err(GenerationError::NoValidGeneration)
        ));
    }
}

#[cfg(windows)]
#[test]
fn windows_junction_directory_is_refused() {
    let t = Fixture::new();
    t.rotate(8443);
    let link = t.dir.with_extension("junction");
    // mklink /J needs no symlink privilege; both paths are isolated fixtures.
    let result = std::process::Command::new("cmd.exe")
        .args(["/c", "mklink", "/J"])
        .arg(&link)
        .arg(&t.dir)
        .output()
        .unwrap();
    assert!(result.status.success(), "junction fixture failed");
    let refused = generation::load(&link, &t.keys.store).is_err();
    fs::remove_dir(&link).unwrap();
    assert!(refused);
}

#[cfg(target_os = "linux")]
#[test]
fn linux_file_and_directory_modes_and_symlinks_are_refused() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    for directory in [false, true] {
        let t = Fixture::new();
        t.rotate(8443);
        let path = if directory {
            t.dir.clone()
        } else {
            t.dir.join(RECORD_FILE)
        };
        fs::set_permissions(
            &path,
            fs::Permissions::from_mode(if directory { 0o755 } else { 0o644 }),
        )
        .unwrap();
        assert!(generation::load(&t.dir, &t.keys.store).is_err());
    }
    let t = Fixture::new();
    t.rotate(8443);
    let path = t.dir.join(RECORD_FILE);
    let moved = t.dir.join("moved");
    fs::rename(&path, &moved).unwrap();
    symlink(&moved, &path).unwrap();
    assert!(generation::load(&t.dir, &t.keys.store).is_err());
}
