fn main() {
    fastframe_i18n::build::compile_catalogs("assets/i18n");

    // Where a release keeps the ICU it ships (the built-in LaTeX links ICU,
    // whose version differs between distributions): `lib/` next to the
    // executable in the archive, `../lib/tectec/` from it once installed.
    // RPATH rather than RUNPATH, so libicuuc finds libicudata there too.
    // Elsewhere (a development build) the system's ICU is found as usual.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        println!("cargo:rustc-link-arg-bins=-Wl,--disable-new-dtags,-rpath,$ORIGIN/lib:$ORIGIN/../lib/tectec");
    }
}
