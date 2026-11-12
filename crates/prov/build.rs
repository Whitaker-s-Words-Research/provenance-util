//! Records the git SHA of the build for the `Prov-Version` trailer.
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=PROV_BUILD_SHA");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    let sha = std::env::var("PROV_BUILD_SHA").ok().filter(|s| !s.is_empty()).or_else(|| {
        let out = Command::new("git").args(["rev-parse", "--short=12", "HEAD"]).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    });
    println!("cargo:rustc-env=PROV_BUILD_SHA={}", sha.unwrap_or_else(|| "unknown".into()));
}
