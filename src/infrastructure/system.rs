pub fn summary() -> String {
    #[cfg(windows)]
    {
        use winreg::{RegKey, enums::HKEY_LOCAL_MACHINE};
        let root = RegKey::predef(HKEY_LOCAL_MACHINE);
        let key = root.open_subkey("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion");
        let (edition, build) = key
            .map(|k| {
                (
                    k.get_value::<String, _>("EditionID").unwrap_or_default(),
                    k.get_value::<String, _>("CurrentBuildNumber")
                        .unwrap_or_default(),
                )
            })
            .unwrap_or_default();
        let family = if build.parse::<u32>().unwrap_or(0) >= 22000 {
            "Windows 11"
        } else {
            "Windows 10"
        };
        let cpu = root
            .open_subkey("HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\0")
            .and_then(|k| k.get_value::<String, _>("ProcessorNameString"))
            .unwrap_or_else(|_| "Processador x64".into());
        let mut mem: windows_sys::Win32::System::SystemInformation::MEMORYSTATUSEX =
            unsafe { std::mem::zeroed() };
        mem.dwLength = std::mem::size_of_val(&mem) as u32;
        unsafe {
            windows_sys::Win32::System::SystemInformation::GlobalMemoryStatusEx(&mut mem);
        }
        let host = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "Este computador".into());
        format!(
            "{host}  •  {family} {edition}  •  build {build}\n{}  •  {:.1} GB de RAM  •  x64",
            cpu.trim(),
            mem.ullTotalPhys as f64 / 1_073_741_824.0
        )
    }
    #[cfg(not(windows))]
    {
        "Sistema não Windows — providers do Windows indisponíveis".into()
    }
}
