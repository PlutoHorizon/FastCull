use std::{env, fs, path::PathBuf, process::Command};

fn resource_compiler() -> PathBuf {
    if let Some(path) = env::var_os("RC") {
        return path.into();
    }
    if let Some(paths) = env::var_os("PATH") {
        for directory in env::split_paths(&paths) {
            let path = directory.join("rc.exe");
            if path.is_file() {
                return path;
            }
        }
    }
    let sdk = env::var_os("WindowsSdkDir")
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("ProgramFiles(x86)").map(|p| PathBuf::from(p).join("Windows Kits/10"))
        })
        .expect("Windows SDK not found; install the Windows SDK or set RC to rc.exe");
    let mut versions: Vec<_> = fs::read_dir(sdk.join("bin"))
        .expect("Windows SDK bin directory not found")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect();
    versions.sort();
    let host = env::var("HOST").unwrap_or_default();
    let arch = if host.starts_with("aarch64") {
        "arm64"
    } else if host.starts_with("i686") {
        "x86"
    } else {
        "x64"
    };
    versions
        .into_iter()
        .rev()
        .map(|version| version.join(arch).join("rc.exe"))
        .find(|path| path.is_file())
        .expect("rc.exe not found; install the Windows SDK or set RC")
}

fn main() {
    println!("cargo:rerun-if-changed=assets/fastcull.ico");
    println!("cargo:rerun-if-env-changed=RC");
    println!("cargo:rerun-if-env-changed=WindowsSdkDir");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    assert_eq!(
        env::var("CARGO_CFG_TARGET_ENV").as_deref(),
        Ok("msvc"),
        "FastCull's Windows build requires the MSVC toolchain"
    );
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let source = out.join("fastcull.rc");
    let resource = out.join("fastcull.res");
    let icon = root
        .join("assets/fastcull.ico")
        .to_string_lossy()
        .replace('\\', "/");
    fs::write(
        &source,
        format!("#pragma code_page(65001)\n1 ICON \"{icon}\"\n"),
    )
    .unwrap();
    let status = Command::new(resource_compiler())
        .arg("/nologo")
        .arg("/fo")
        .arg(&resource)
        .arg(&source)
        .status()
        .expect("Failed to run Windows resource compiler");
    assert!(
        status.success(),
        "Failed to compile FastCull's Windows icon"
    );
    println!("cargo:rustc-link-arg-bin=fastcull={}", resource.display());
}
