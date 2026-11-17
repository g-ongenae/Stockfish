//! What the C++ Makefile passes to the compiler for `engine_version_info()`
//! and `compiler_info()` (`misc.cpp`): the date and commit of the checkout
//! (`GIT_DATE`, `GIT_SHA`, and `GIT_DIFFINDEX` when the tree differs from the
//! commit), and here the version of the Rust compiler. Only the standard
//! library; nothing is downloaded.

use std::process::Command;

fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// The civil date (proleptic Gregorian) of a day count since 1970-01-01.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn main() {
    // Rerun on a commit, a checkout, a change of the index, or any change to
    // the sources of the package
    for path in ["HEAD", "index"] {
        if let Some(p) = run("git", &["rev-parse", "--git-path", path]) {
            println!("cargo:rerun-if-changed={p}");
        }
    }
    if let Some(r) = run("git", &["symbolic-ref", "-q", "HEAD"]) {
        if let Some(p) = run("git", &["rev-parse", "--git-path", &r]) {
            println!("cargo:rerun-if-changed={p}");
        }
    }
    for path in ["src", "Cargo.toml", "build.rs"] {
        println!("cargo:rerun-if-changed={path}");
    }

    // GIT_SHA: the first 8 characters of the commit
    let sha = run("git", &["rev-parse", "HEAD"]).map(|s| s.chars().take(8).collect::<String>());
    // GIT_DATE: the date of the commit, else (as __DATE__) that of the build
    let date = run("git", &["show", "-s", "--date=format:%Y%m%d", "--format=%cd", "HEAD"]).unwrap_or_else(|| {
        let secs =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        let (y, m, d) = civil_from_days(secs.div_euclid(86400));
        format!("{y:04}{m:02}{d:02}")
    });
    // GIT_DIFFINDEX: the tree differs from the commit
    let _ = Command::new("git").args(["update-index", "--refresh"]).output();
    let modified = Command::new("git")
        .args(["-c", "core.filemode=false", "-c", "core.autocrlf=false", "diff-index", "--quiet", "HEAD", "--"])
        .status()
        .map(|s| s.code() == Some(1))
        .unwrap_or(false);

    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let rustc_version = run(&rustc, &["-V"]).unwrap_or_else(|| "(unknown version)".into());

    println!("cargo:rustc-env=SF_GIT_DATE={date}");
    println!("cargo:rustc-env=SF_GIT_SHA={}", sha.unwrap_or_else(|| "nogit".into()));
    println!("cargo:rustc-env=SF_GIT_DIFFINDEX={}", if modified { "1" } else { "" });
    println!("cargo:rustc-env=SF_RUSTC_VERSION={rustc_version}");
    println!("cargo:rustc-env=SF_TARGET={}", std::env::var("TARGET").unwrap_or_default());
}
