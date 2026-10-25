pub struct Target;

impl super::ClangRuntime for Target {
    fn link(&self) {
        // Rust links with -nodefaultlibs, while Whisper's Metal objects use this Clang runtime symbol.
        let output = std::process::Command::new("xcrun")
            .args(["clang", "-print-resource-dir"])
            .output()
            .expect("failed to locate the macOS Clang runtime");
        assert!(output.status.success(), "xcrun clang -print-resource-dir failed");
        let resource_dir = String::from_utf8(output.stdout).expect("invalid Clang resource directory");
        let archive = std::path::Path::new(resource_dir.trim()).join("lib/darwin/libclang_rt.osx.a");
        assert!(archive.is_file(), "missing Clang runtime: {}", archive.display());
        println!("cargo:rustc-link-arg={}", archive.display());
    }
}
