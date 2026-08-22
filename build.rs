fn main() {
    slint_build::compile("ui/main.slint").expect("compile Slint user interface");

    #[cfg(windows)]
    {
        let manifest = r#"
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false" />
      </requestedPrivileges>
    </security>
  </trustInfo>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}" />
      <supportedOS Id="{4f476546-9374-4f8f-9a2a-531e8f92e65a}" />
    </application>
  </compatibility>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
      <longPathAware xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">true</longPathAware>
    </windowsSettings>
  </application>
</assembly>
"#;
        // Forward slashes: backslashes are escape characters inside .rc string literals.
        let icon = format!(
            "{}/ui/assets/icon.ico",
            std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR")
        )
        .replace('\\', "/");
        println!("cargo:rerun-if-changed=ui/assets/icon.ico");

        winresource::WindowsResource::new()
            .set_icon(&icon)
            .set_manifest(manifest)
            .set("FileDescription", "OBR Music Tool")
            .set("ProductName", "Oblivion Remastered Music Tool")
            .set("OriginalFilename", "OBR Music Tool.exe")
            .set("CompanyName", "Computer Works")
            .set(
                "LegalCopyright",
                "Copyright (c) 2026 Lorex (LorexValkin). Free and open-source software, OBR Music Tool License.",
            )
            .set(
                "Comments",
                "Free, open-source tool by Lorex; no paywalls. Release builds are code-signed by Computer Works, a company owned by the developer. Not affiliated with Bethesda Softworks, Virtuos or Audiokinetic. Source: https://github.com/LorexValkin/OBR-Music-Tool",
            )
            .compile()
            .expect("compile Windows application resources");
    }
}
