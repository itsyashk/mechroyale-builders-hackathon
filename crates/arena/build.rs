//! Compiles zig/geom.zig into a static library with the pinned Zig toolchain.
use std::{env, path::PathBuf, process::Command};

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let src = manifest.join("../../zig/geom.zig").canonicalize().expect("zig/geom.zig");
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let target = env::var("TARGET").unwrap();
    let zig_target = match target.as_str() {
        "x86_64-pc-windows-msvc" => "x86_64-windows-msvc",
        "x86_64-unknown-linux-gnu" => "x86_64-linux-gnu",
        "x86_64-unknown-linux-musl" => "x86_64-linux-musl",
        "aarch64-unknown-linux-gnu" => "aarch64-linux-gnu",
        other => panic!("unsupported target {other}"),
    };
    let msvc = target.ends_with("msvc");
    let lib = out.join(if msvc { "geom.lib" } else { "libgeom.a" });
    let zig = env::var("ZIG").unwrap_or_else(|_| "zig".into());
    let cache = out.join("zig-cache");
    let mut cmd = Command::new(&zig);
    cmd.arg("build-lib")
        .arg("-OReleaseFast")
        .arg("-target")
        .arg(zig_target)
        .arg("-fno-stack-check")
        .arg("-fno-compiler-rt")
        .arg("--cache-dir")
        .arg(&cache)
        .arg("--global-cache-dir")
        .arg(&cache)
        .arg(format!("-femit-bin={}", lib.display()))
        .arg(&src);
    if !msvc {
        cmd.arg("-fPIC");
    }
    let st = cmd.status().unwrap_or_else(|e| panic!("running {zig}: {e} (set ZIG=/path/to/zig)"));
    assert!(st.success(), "zig build-lib failed");
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=geom");
    println!("cargo:rerun-if-changed={}", src.display());
    println!("cargo:rerun-if-env-changed=ZIG");
}
