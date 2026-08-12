fn main() {
    #[cfg(target_os = "macos")]
    {
        cc::Build::new()
            .file("macos/notification.m")
            .flag("-fobjc-arc")
            .compile("zlack_macos_notification");
        println!("cargo:rustc-link-lib=framework=Foundation");
    }

    tauri_build::build()
}
