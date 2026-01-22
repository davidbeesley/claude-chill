use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");

    let git_hash = get_git_hash();
    println!("cargo:rustc-env=GIT_HASH={}", git_hash);
}

fn get_git_hash() -> String {
    let hash = match Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
    {
        Ok(output) if output.status.success() => String::from_utf8(output.stdout)
            .ok()
            .map(|s| s.trim().to_string()),
        _ => None,
    };

    let Some(hash) = hash else {
        return "git not found during build, version unavailable".to_string();
    };

    let is_dirty = Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .ok()
        .map(|output| !output.stdout.is_empty())
        .unwrap_or(false);

    if is_dirty {
        format!("{}-dirty", hash)
    } else {
        hash
    }
}
