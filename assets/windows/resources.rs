//! Shared build-only Windows resources. No manifest: preserve the toolchain/GPUI
//! DPI and subsystem behavior. Per-binary linking avoids rlib resource collisions.
use std::{env, fs, path::PathBuf};

pub fn compile(binaries: &[&str]) {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("../..");
    let icon = root.join("assets/icon/pod.ico").canonicalize().unwrap();
    println!("cargo:rerun-if-changed={}", icon.display());
    println!(
        "cargo:rerun-if-changed={}",
        root.join("assets/windows/resources.rs").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        root.join("Cargo.toml").display()
    );
    // All release binary packages inherit workspace.package.version.
    let version = env::var("CARGO_PKG_VERSION").unwrap();
    let numeric = ["MAJOR", "MINOR", "PATCH"].map(|part| {
        env::var(format!("CARGO_PKG_VERSION_{part}"))
            .unwrap()
            .parse::<u16>()
            .unwrap()
    });
    let quad = format!("{},{},{},0", numeric[0], numeric[1], numeric[2]);
    let icon = icon
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .replace('\\', "/");
    for binary in binaries {
        let description = match *binary {
            "pod-app" => "Pod agent coding IDE (Apache-2.0)",
            "podd" => "Pod terminal daemon (Apache-2.0)",
            "pod-holder" => "Pod terminal host (Apache-2.0)",
            "pod" => "Pod command line (Apache-2.0)",
            "pod-hook" => "Pod agent hook relay (Apache-2.0)",
            _ => unreachable!(),
        };
        let rc = format!(
            r#"
#pragma code_page(65001)
1 ICON "{icon}"
1 VERSIONINFO
FILEVERSION {quad}
PRODUCTVERSION {quad}
FILEFLAGSMASK 0x3fL
FILEFLAGS 0
FILEOS 0x40004L
FILETYPE 1
FILESUBTYPE 0
BEGIN
 BLOCK "StringFileInfo"
 BEGIN
  BLOCK "040904b0"
  BEGIN
   VALUE "CompanyName", "Linejin\0"
   VALUE "FileDescription", "{description}\0"
   VALUE "FileVersion", "{version}\0"
   VALUE "InternalName", "{binary}\0"
   VALUE "LegalCopyright", "Copyright (c) 2026 Linejin. Apache-2.0.\0"
   VALUE "OriginalFilename", "{binary}.exe\0"
   VALUE "ProductName", "Pod\0"
   VALUE "ProductVersion", "{version}\0"
   VALUE "Comments", "Licensed under the Apache License, Version 2.0\0"
  END
 END
 BLOCK "VarFileInfo"
 BEGIN
  VALUE "Translation", 0x0409, 1200
 END
END
"#
        );
        let path = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join(format!("{binary}.rc"));
        fs::write(&path, rc).unwrap();
        embed_resource::compile_for(path, [binary], embed_resource::NONE)
            .manifest_required()
            .expect("Pod Windows resources must compile");
    }
}
