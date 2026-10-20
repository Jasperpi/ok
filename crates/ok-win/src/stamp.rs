//! Writing a manifest and icons into a finished exe.
//!
//! There is no resource compiler on the build machine, and a running exe
//! cannot change itself, so this is a step after the build: copy the exe and
//! let Windows write the resources into the copy.

use std::ffi::c_void;
use std::io;
use std::path::Path;

use crate::win32::*;
use crate::Icon;

/// The manifest ok ships with. It says three things: the process sees each
/// monitor's real pixels (per-monitor DPI awareness, version 2), it knows
/// Windows 10 (so Windows reports its true version to it), and it may use
/// paths longer than 260 characters.
pub const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0" xmlns:asmv3="urn:schemas-microsoft-com:asm.v3">
  <assemblyIdentity type="win32" name="JasperMeller.ok" version="0.1.0.0"/>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/>
    </application>
  </compatibility>
  <asmv3:application>
    <asmv3:windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2, PerMonitor</dpiAwareness>
      <longPathAware xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">true</longPathAware>
    </asmv3:windowsSettings>
  </asmv3:application>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
</assembly>
"#;

/// The language the resources are filed under: US English, as the probe
/// wrote them and Windows read them back.
const LANGUAGE: u16 = 0x0409;

/// A newly copied exe is often still open in the virus scanner. How many
/// times to try, and how long to wait between tries.
const TRIES: u32 = 8;
const PAUSE_MS: u32 = 250;

/// Copies `src_exe` to `dst_exe` and writes `manifest` and `icons` into the
/// copy as its application manifest and its icon. Explorer, the taskbar and
/// Alt Tab then show that icon for the exe.
///
/// An empty `manifest` or an empty `icons` leaves that resource out.
pub fn stamp(src_exe: &Path, dst_exe: &Path, icons: &[Icon], manifest: &str) -> io::Result<()> {
    if let Some(bad) = icons.iter().find(|i| !i.is_well_formed()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("an icon of {} by {} needs exactly that many pixels and a side of 1 to 256", bad.width, bad.height),
        ));
    }
    std::fs::copy(src_exe, dst_exe)?;
    let mut last = io::Error::other("the resources could not be written");
    for attempt in 0..TRIES {
        if attempt > 0 {
            unsafe { Sleep(PAUSE_MS) };
        }
        match write_resources(dst_exe, icons, manifest) {
            Ok(()) => return Ok(()),
            Err(e) => last = e,
        }
    }
    Err(last)
}

fn write_resources(exe: &Path, icons: &[Icon], manifest: &str) -> io::Result<()> {
    unsafe {
        let update = BeginUpdateResourceW(wide_path(exe).as_ptr(), FALSE);
        if update == 0 {
            return Err(io::Error::last_os_error());
        }
        let put = |kind: PCWSTR, id: u16, bytes: &[u8]| -> bool {
            UpdateResourceW(update, kind, make_int_resource(id), LANGUAGE, bytes.as_ptr() as *const c_void, bytes.len() as u32) != 0
        };
        let mut ok = manifest.is_empty() || put(RT_MANIFEST, 1, manifest.as_bytes());
        // Each image is its own resource, numbered from 1; the group lists them.
        let mut listed = Vec::new();
        for (n, icon) in icons.iter().enumerate() {
            let (w, h) = (icon.width as i32, icon.height as i32);
            let image = icon_resource_bytes(w, h, &icon.argb);
            ok = ok && put(RT_ICON, n as u16 + 1, &image);
            listed.push((w, h, image.len()));
        }
        if !icons.is_empty() {
            ok = ok && put(RT_GROUP_ICON, 1, &icon_group_bytes(&listed));
        }
        if !ok {
            let failure = io::Error::last_os_error();
            EndUpdateResourceW(update, TRUE);
            return Err(failure);
        }
        if EndUpdateResourceW(update, FALSE) == 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bytes of one resource in an exe that is opened as data, not run.
    fn resource(module: HMODULE, kind: PCWSTR, id: u16) -> Option<Vec<u8>> {
        unsafe {
            let found = FindResourceW(module, make_int_resource(id), kind);
            if found == 0 {
                return None;
            }
            let size = SizeofResource(module, found) as usize;
            let bytes = LockResource(LoadResource(module, found)) as *const u8;
            if bytes.is_null() {
                return None;
            }
            Some(std::slice::from_raw_parts(bytes, size).to_vec())
        }
    }

    fn test_icon(side: u32) -> Icon {
        let argb = (0..side * side).map(|i| if i % side < side / 2 { 0xFFE7_C9A9 } else { 0x8024_1E19 }).collect();
        Icon { width: side, height: side, argb }
    }

    #[test]
    fn a_stamped_copy_carries_the_manifest_and_icons_and_still_runs() {
        let dir = crate::scratch("stamp");
        let src = std::env::current_exe().expect("this test's own exe");
        let dst = dir.join("stamped.exe");
        let icons = [test_icon(32), test_icon(16)];
        stamp(&src, &dst, &icons, MANIFEST).expect("stamped");
        println!("{} bytes before, {} after", std::fs::metadata(&src).unwrap().len(), std::fs::metadata(&dst).unwrap().len());

        let module = unsafe { LoadLibraryExW(wide_path(&dst).as_ptr(), 0, LOAD_LIBRARY_AS_DATAFILE) };
        assert_ne!(module, 0, "the stamped exe opens as a module");
        let manifest = resource(module, RT_MANIFEST, 1);
        let group = resource(module, RT_GROUP_ICON, 1);
        let (big, small) = (resource(module, RT_ICON, 1), resource(module, RT_ICON, 2));
        unsafe { FreeLibrary(module) };

        assert_eq!(manifest.as_deref(), Some(MANIFEST.as_bytes()));
        let group = group.expect("an icon group");
        assert_eq!(&group[..6], &[0, 0, 1, 0, 2, 0], "an icon directory of two images");
        assert_eq!((group[6], group[7]), (32, 32));
        // 40 bytes of header, the pixels, then a one-bit mask padded to 32 bits a row.
        assert_eq!(big.expect("the 32 pixel image").len(), 40 + 32 * 32 * 4 + 32 * 4);
        let small = small.expect("the 16 pixel image");
        assert_eq!(small.len(), 40 + 16 * 16 * 4 + 16 * 4);
        // Pixels are stored bottom row first, blue first: the last row's first pixel is ours.
        assert_eq!(&small[40..44], &0xFFE7_C9A9u32.to_le_bytes());

        // The copy is still a program: ask it to list its tests.
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let listed = std::process::Command::new(&dst).arg("--list").creation_flags(CREATE_NO_WINDOW).output().expect("the stamped exe starts");
        assert!(listed.status.success());
        assert!(String::from_utf8_lossy(&listed.stdout).contains("a_stamped_copy_carries_the_manifest"));
    }

    #[test]
    fn a_malformed_icon_is_refused_before_anything_is_copied() {
        let dir = crate::scratch("stamp-refused");
        let dst = dir.join("never.exe");
        let short = Icon { width: 16, height: 16, argb: vec![0; 10] };
        let src = std::env::current_exe().expect("this test's own exe");
        let err = stamp(&src, &dst, &[short], MANIFEST).expect_err("refused");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(!dst.exists());
        let huge = Icon { width: 300, height: 300, argb: vec![0; 90_000] };
        assert!(stamp(&src, &dst, &[huge], MANIFEST).is_err());
    }

    #[test]
    fn a_missing_source_is_an_error() {
        let dir = crate::scratch("stamp-missing");
        assert!(stamp(&dir.join("no-such.exe"), &dir.join("out.exe"), &[], MANIFEST).is_err());
    }
}
