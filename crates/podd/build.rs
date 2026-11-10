//! Copies the bundled ConPTY (`vendor/conpty`, fetched by
//! `tools/fetch-conpty.ps1`) next to the built binaries so `pod-holder` finds
//! `conpty.dll` + `x64\OpenConsole.exe` in its own directory. Without it the
//! holder falls back to the inbox ConPTY.

use std::path::{Path, PathBuf};

#[path = "../../assets/windows/resources.rs"]
mod resources;

fn main() {
    resources::compile(&["podd", "pod-holder"]);
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let vendor = manifest.join("../../vendor/conpty");
    println!("cargo:rerun-if-changed={}", vendor.display());
    println!(
        "cargo:rerun-if-changed={}",
        vendor.join("conpty.dll").display()
    );
    if !vendor.join("conpty.dll").is_file() {
        println!(
            "cargo:warning=vendor/conpty missing: pod-holder will use the inbox ConPTY (run tools/fetch-conpty.ps1)"
        );
        return;
    }
    // OUT_DIR = <target>/<profile>/build/podd-<hash>/out
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let Some(profile_dir) = out.ancestors().nth(3) else {
        return;
    };
    for dir in [profile_dir.to_path_buf(), profile_dir.join("deps")] {
        copy_tree(&vendor, &dir);
    }
}

fn copy_tree(from: &Path, to: &Path) {
    let Ok(entries) = std::fs::read_dir(from) else {
        return;
    };
    let _ = std::fs::create_dir_all(to);
    for e in entries.flatten() {
        let p = e.path();
        let name = e.file_name();
        if p.is_dir() {
            copy_tree(&p, &to.join(&name));
        } else if p.extension().is_some_and(|x| x == "dll" || x == "exe") {
            let dst = to.join(&name);
            let same = std::fs::metadata(&dst).ok().map(|m| m.len())
                == std::fs::metadata(&p).ok().map(|m| m.len());
            if !same {
                // A running holder may lock the file; that copy is then already current.
                let _ = std::fs::copy(&p, &dst);
            }
        }
    }
}
