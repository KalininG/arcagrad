//! Builds and stages the bundled WASM plugins.

use std::path::Path;
use std::process::Command;

// Keep in sync with wasm_host::BUNDLED.
const PLUGINS: &[&str] = &[
    "anilist",
    "comicvine",
    "openlibrary",
    "gutenberg",
    "marxists",
    "viz",
];

fn main() {
    let root = env!("CARGO_MANIFEST_DIR");

    let home = std::env::var("HOME").unwrap_or_default();
    let cargo_home = std::env::var("CARGO_HOME").unwrap_or_else(|_| format!("{home}/.cargo"));
    let rustflags = format!("--remap-path-prefix={cargo_home}=/cargo --remap-path-prefix={home}=~");

    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());

    for name in PLUGINS {
        let dir = format!("{root}/plugins/{name}");
        let manifest = format!("{dir}/Cargo.toml");

        // cargo-chef runs this against a source-less skeleton.
        if !Path::new(&manifest).exists() {
            println!("cargo:warning=plugin source {name} not found; skipping wasm build");
            continue;
        }

        println!("cargo:rerun-if-changed={dir}/src");
        println!("cargo:rerun-if-changed={manifest}");

        let status = Command::new(&cargo)
            .args([
                "build",
                "--release",
                "--target",
                "wasm32-unknown-unknown",
                "--manifest-path",
                &manifest,
            ])
            // Encoded parent flags override RUSTFLAGS and may contain a native linker.
            .env_remove("CARGO_ENCODED_RUSTFLAGS")
            .env("RUSTFLAGS", &rustflags)
            // The staging path assumes each plugin's own target directory.
            .env_remove("CARGO_TARGET_DIR")
            .status()
            .unwrap_or_else(|e| panic!("failed to spawn cargo for plugin '{name}': {e}"));

        assert!(
            status.success(),
            "building plugin '{name}' to wasm failed.\n\
             Ensure the target is installed:  rustup target add wasm32-unknown-unknown"
        );

        let built = format!("{dir}/target/wasm32-unknown-unknown/release/{name}.wasm");
        let dest = format!("{dir}/{name}.wasm");
        std::fs::copy(&built, &dest)
            .unwrap_or_else(|e| panic!("staging {name}.wasm ({built} -> {dest}): {e}"));
    }
}
