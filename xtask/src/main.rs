//! 빌드 도구: 화면 코드(sai-ui, Rust)를 WebAssembly 로 만들어 ui/pkg 에 넣는다.
//!   cargo run -p xtask -- ui            (릴리스)
//!   cargo run -p xtask -- ui --dev      (개발용, 빠르게)
//! wasm32 용 Rust 가 다른 곳에 있으면 SAI_WASM_CARGO / SAI_WASM_RUSTC 로 지정한다.
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(|s| s.as_str()) {
        Some("ui") => build_ui(args.iter().any(|a| a == "--dev")),
        _ => {
            eprintln!("사용법: cargo run -p xtask -- ui [--dev]");
            std::process::exit(2);
        }
    }
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

fn build_ui(dev: bool) {
    let root = root();
    let target_dir = root.join("target").join("wasm");
    let cargo = std::env::var("SAI_WASM_CARGO").unwrap_or_else(|_| "cargo".into());
    let mut cmd = Command::new(&cargo);
    cmd.current_dir(&root).args(["build", "-p", "sai-ui", "--lib", "--target", "wasm32-unknown-unknown"]).env("CARGO_TARGET_DIR", &target_dir).env("CARGO_PROFILE_RELEASE_STRIP", "false");
    if !dev {
        cmd.arg("--release");
    }
    if let Ok(rustc) = std::env::var("SAI_WASM_RUSTC") {
        cmd.env("RUSTC", rustc);
    }
    // 바깥 cargo 가 넘긴 설정이 섞이지 않게 한다
    for k in ["RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_TARGET"] {
        cmd.env_remove(k);
    }
    let status = cmd.status().expect("cargo 를 실행하지 못했어요");
    if !status.success() {
        eprintln!("화면(sai-ui)을 WebAssembly 로 만들지 못했어요.");
        std::process::exit(1);
    }
    let wasm = target_dir.join("wasm32-unknown-unknown").join(if dev { "debug" } else { "release" }).join("sai_ui.wasm");
    let out = root.join("ui").join("pkg");
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out).unwrap();
    wasm_bindgen_cli_support::Bindgen::new()
        .input_path(&wasm)
        .web(true)
        .expect("web 대상")
        .typescript(false)
        .debug(dev)
        .generate(&out)
        .expect("wasm-bindgen 처리에 실패했어요");
    // 화면이 불러가는 시작 파일 (자동으로 만듦: WebAssembly 를 불러와 Rust 화면 코드를 시작한다)
    std::fs::write(out.join("boot.js"), "// 자동 생성 파일 — 고치지 마세요 (cargo run -p xtask -- ui)\nimport init from './sai_ui.js';\ninit({ module_or_path: new URL('./sai_ui_bg.wasm', import.meta.url) });\n").unwrap();
    println!("화면을 만들었어요: {}", out.display());
}
