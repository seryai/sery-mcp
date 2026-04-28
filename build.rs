//! Build script: link Windows-only libraries the bundled SQL engine
//! references but doesn't link itself.
//!
//! On Windows, the bundled C++ source calls the Restart Manager APIs
//! (`RmStartSession` / `RmEndSession` / `RmRegisterResources` /
//! `RmGetList`) without instructing cargo to link `Rstrtmgr.lib`. The
//! resulting `link.exe` invocation hits LNK2019 unresolved-external
//! errors. Adding the directive ourselves at the consumer level works
//! around it without forking the dep.
//!
//! Linux + macOS need no equivalent — the bundled C++ build there
//! statically links everything it needs.

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "windows" {
        println!("cargo:rustc-link-lib=Rstrtmgr");
    }
}
