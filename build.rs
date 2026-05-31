// build.rs — embeds app.manifest into the final .exe via the MSVC linker.
//
// We use the linker's /MANIFEST:EMBED + /MANIFESTINPUT switches rather than pulling in a
// resource-compiler crate (embed-resource, winres) to honour the "windows-rs only" rule from
// the spec.

fn main() {
    println!("cargo:rerun-if-changed=app.manifest");
    println!("cargo:rerun-if-changed=build.rs");

    // Only emit the linker args when targeting the MSVC toolchain (the only supported target).
    let target = std::env::var("TARGET").unwrap_or_default();
    if !target.contains("msvc") {
        return;
    }

    let manifest_path = std::env::current_dir()
        .expect("cwd")
        .join("app.manifest");
    let manifest_str = manifest_path.display().to_string();

    println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}", manifest_str);
    println!("cargo:rustc-link-arg-bins=/MANIFESTUAC:level='asInvoker' uiAccess='false'");
}
