//! 只在 Windows 上：把应用图标与版本信息（VERSIONINFO）嵌进 `pdfbatch.exe`，
//! 这样资源管理器、任务栏、开始菜单与 Alt+Tab 才会显示我们自己的图标。
//!
//! 其他平台什么都不做；找不到资源编译器时只给一条 warning，不阻断构建。

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../assets/app-icon/pdfbatch.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("../../assets/app-icon/pdfbatch.ico")
        .set("ProductName", "pdfbatch")
        .set("FileDescription", "pdfbatch - batch fill layers and bookmarks for PDFs")
        .set("LegalCopyright", "MIT OR Apache-2.0")
        .set("OriginalFilename", "pdfbatch.exe")
        .set("InternalName", "pdfbatch");
    if let Err(error) = res.compile() {
        println!("cargo:warning=pdfbatch.exe 未嵌入图标/版本资源：{error}");
    }
}