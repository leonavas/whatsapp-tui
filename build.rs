//! Builds the WhatsApp bridge (bridge/, Go + whatsmeow) into
//! `$OUT_DIR/whatsapp-tui-bridge`, for `bridge.rs` to carry inside whatsapp-tui.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    let env = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("{k} not set"));
    let root = PathBuf::from(env("CARGO_MANIFEST_DIR"));
    let out = PathBuf::from(env("OUT_DIR")).join("whatsapp-tui-bridge");
    let release = env("PROFILE") == "release";

    let mut args: Vec<String> = vec!["build".into(), "-trimpath".into()];
    if release {
        args.extend(["-ldflags".into(), "-s -w".into()]);
    }
    args.extend(["-o".into(), out.display().to_string(), ".".into()]);

    // `go` from PATH, or through mise (mise.toml pins it) when it isn't there.
    let run = |prog: &str, pre: &[&str]| {
        Command::new(prog)
            .args(pre)
            .args(&args)
            .current_dir(root.join("bridge"))
            .env("CGO_ENABLED", "1")
            .status()
    };
    let status = run("go", &[]).or_else(|_| run("mise", &["exec", "--", "go"])).expect("go build of bridge/ (install Go, or mise)");
    assert!(status.success(), "building the WhatsApp bridge (bridge/) failed");

    for p in ["bridge/main.go", "bridge/biz.go", "bridge/media.go", "bridge/download.go", "bridge/chats.go", "bridge/go.mod", "bridge/go.sum", "build.rs"] {
        println!("cargo:rerun-if-changed={p}");
    }
}
