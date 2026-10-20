fn main() {
    slint_build::compile("ui/app.slint").expect("failed to compile the .slint");

    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=../../assets/icons/kalkan-monitor.ico");
        winresource::WindowsResource::new()
            .set_icon("../../assets/icons/kalkan-monitor.ico")
            .compile()
            .expect("failed to embed the Windows icon");
    }
}
