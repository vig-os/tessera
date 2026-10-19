//! Build script — extension-module link arguments for the `_native` cdylib (#647).
//!
//! `extension-module` stops pyo3 linking `libpython`: the interpreter that imports the module
//! provides the `_Py*` symbols at load time. ELF linkers accept a shared object with undefined
//! symbols, but Apple's `ld` rejects it unless the dylib is linked with `-undefined dynamic_lookup`.
//! pyo3 cannot add that flag itself: `rustc-cdylib-link-arg` only applies to the cdylib of the crate
//! whose build script emits it. maturin passes it for its own builds, but a plain
//! `cargo build -p tessera-py` on macOS failed with `Undefined symbols … "_PyBaseObject_Type"`.
//!
//! This is pyo3's documented mechanism for builds not driven by maturin ("Building and
//! distribution → macOS"). It emits `-undefined dynamic_lookup` for Darwin targets only and is a
//! no-op everywhere else, so the Linux `.so` and the reproducible nix wheel are unchanged.

fn main() {
    pyo3_build_config::add_extension_module_link_args();
}
