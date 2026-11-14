//! Passes the target the library is built for (`x86_64-pc-windows-gnu` and
//! the like) to the code as `TFLIP_TARGET`, for `AdsGetVersion`'s
//! description; Cargo gives it to build scripts only.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let target = std::env::var("TARGET").expect("Cargo sets TARGET for build scripts");
    println!("cargo:rustc-env=TFLIP_TARGET={target}");
}
