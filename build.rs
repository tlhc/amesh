use std::{path::Path, process::Command};

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    output.status.success().then_some(())?;
    Some(String::from_utf8(output.stdout).ok()?.trim_end().to_owned())
}

fn main() {
    if git(&["rev-parse", "--show-prefix"]).as_deref() != Some("") {
        return;
    }
    let head_ref = git(&["symbolic-ref", "-q", "HEAD"]);
    for name in ["HEAD", "packed-refs"]
        .into_iter()
        .chain(head_ref.as_deref())
    {
        if let Some(path) = git(&["rev-parse", "--path-format=absolute", "--git-path", name]) {
            if head_ref.as_deref() == Some(name) || Path::new(&path).exists() {
                println!("cargo::rerun-if-changed={path}");
            }
        }
    }
    if let Some(revision) = git(&["rev-parse", "--verify", "HEAD"]) {
        println!("cargo::rustc-env=AMESH_GIT_REVISION={revision}");
    }
}
