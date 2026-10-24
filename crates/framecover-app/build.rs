//! Compiles the Slint UI and, on Windows, embeds the executable icon and
//! application manifest.

const WINDOWS_MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity type="win32" name="FrameCover.Application" version="1.0.0.0"/>
  <dependency>
    <dependentAssembly>
      <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls"
        version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/>
    </dependentAssembly>
  </dependency>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
      <activeCodePage xmlns="http://schemas.microsoft.com/SMI/2019/WindowsSettings">UTF-8</activeCodePage>
    </windowsSettings>
  </application>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/>
    </application>
  </compatibility>
</assembly>
"#;

fn main() {
    slint_build::compile("ui/app.slint").expect("Slint UI failed to compile");

    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=../../icon.ico");
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../icon.ico")
            .set("FileDescription", "FrameCover")
            .set("ProductName", "FrameCover")
            .set("CompanyName", "Fedor Gagarin")
            .set(
                "LegalCopyright",
                "Copyright (c) Fedor Gagarin. MIT License.",
            )
            .set_manifest(WINDOWS_MANIFEST);
        if let Err(err) = res.compile() {
            println!("cargo:warning=could not embed Windows resources: {err}");
        }
    }
}
