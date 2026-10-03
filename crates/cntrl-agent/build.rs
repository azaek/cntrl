//! Records the target triple. The agent reports it in every hello, so Console can
//! offer the release artifact built for it.

fn main() {
    if let Ok(target) = std::env::var("TARGET") {
        println!("cargo:rustc-env=CNTRL_TARGET={target}");
    }
}
