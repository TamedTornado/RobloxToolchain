use roblox_toolchain::scripts::{Config, compile, compile_file};
use std::fs;

fn config() -> Config {
    Config {
        optimization_level: 1,
        debug_level: 1,
        type_info_level: 0,
        coverage_level: 0,
    }
}

#[test]
fn luau_compilation_is_deterministic_and_never_executes_source() {
    let source = "local value: number = 4\nerror('this must not execute')\nreturn value";
    let bytes = compile(source, &config()).unwrap();
    assert!(!bytes.is_empty());
    assert_eq!(bytes, compile(source, &config()).unwrap());
    let mut different = config();
    different.debug_level = 2;
    assert_ne!(bytes, compile(source, &different).unwrap());
    different.optimization_level = 3;
    assert!(compile(source, &different).is_err());
    assert!(compile("local = invalid", &config()).is_err());
}

#[test]
fn compile_cli_rejects_invalid_source_and_preserves_existing_output() {
    let temporary = tempfile::tempdir().unwrap();
    let source = temporary.path().join("script.luau");
    let output = temporary.path().join("script.luauc");
    fs::write(&source, "return 42").unwrap();
    let result = std::process::Command::new(env!("CARGO_BIN_EXE_roblox"))
        .env_clear()
        .arg("compile")
        .arg(&source)
        .args(["--config", "examples/compiler.json", "--output"])
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(
        value["result"]["robloxDeploymentCompatibilityVerified"],
        false
    );
    let bytes = fs::read(&output).unwrap();
    assert!(compile_file(&source, &output, &config()).is_err());
    assert_eq!(bytes, fs::read(&output).unwrap());
    fs::write(&source, "local =").unwrap();
    let invalid = temporary.path().join("invalid.luauc");
    assert!(compile_file(&source, &invalid, &config()).is_err());
    assert!(!invalid.exists());
}
