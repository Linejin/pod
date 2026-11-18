//! Copies the bundled ConPTY (`vendor/conpty`, fetched by
//! `tools/fetch-conpty.ps1`) next to the built binaries so `pod-holder` finds
//! `conpty.dll` + `x64\OpenConsole.exe` in its own directory. Without it the
//! holder falls back to the inbox ConPTY.

use std::path::PathBuf;

mod build_support;
use build_support::{copy_tree, profile_dir};

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
    // Cargo supports both build/podd-<hash>/out and
    // build/podd/<hash>/out. Locate the profile by the build directory,
    // rather than assuming a fixed number of ancestors.
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let Some(profile_dir) = profile_dir(&out) else {
        return;
    };
    for dir in [profile_dir.to_path_buf(), profile_dir.join("deps")] {
        copy_tree(&vendor, &dir).expect("cannot copy bundled ConPTY");
    }
}
