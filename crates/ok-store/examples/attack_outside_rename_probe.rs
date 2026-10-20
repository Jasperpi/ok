//! A measurement, with no store in it: how long is the gap between "look at
//! the file under the name" and "rename my temp file over that name", when
//! other programs are at the same file?
//!
//! One thread saves the way the store does (deny-write guard on the target,
//! temp file written and flushed, the target looked at by path, then the
//! temp renamed over it through its own handle). Another replaces the target
//! by writing a temp file and renaming it over. With the argument `holders`,
//! two more open the target in random access and share modes. Every time
//! the first thread's rename succeeds although the other program's file had
//! taken the name after the look, that is a version replaced unseen; the
//! probe counts them and reports how long each look-to-rename took.
//!
//! Run: `attack_outside_rename_probe [holders] [seconds]`. It writes only
//! under the build's own scratch folder.

use std::ffi::c_void;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ok_store::testing::Scratch;

#[repr(C)]
#[derive(Default)]
struct ByHandle {
    attrs: u32,
    created: [u32; 2],
    accessed: [u32; 2],
    written: [u32; 2],
    volume: u32,
    size_high: u32,
    size_low: u32,
    links: u32,
    index_high: u32,
    index_low: u32,
}

#[link(name = "kernel32")]
extern "system" {
    fn GetLastError() -> u32;
    fn GetFileInformationByHandle(handle: isize, info: *mut ByHandle) -> i32;
    fn SetFileInformationByHandle(handle: isize, class: u32, info: *const c_void, size: u32) -> i32;
}

fn index_of(file: &File) -> Option<u64> {
    let mut info = ByHandle::default();
    // SAFETY: `info` is a live BY_HANDLE_FILE_INFORMATION and the handle is open.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle() as isize, &mut info) } == 0 {
        return None;
    }
    Some(((info.index_high as u64) << 32) | info.index_low as u64)
}

/// The index of the file under `path`, through a handle that asks for no access.
fn look(path: &Path) -> Option<u64> {
    index_of(&OpenOptions::new().access_mode(0).share_mode(7).custom_flags(0x0200_0000).open(path).ok()?)
}

/// Renames the file behind `file` over `to` (an absolute verbatim path) in
/// one step, replacing what is there even if others have it open.
fn rename_over(file: &File, to: &Path) -> Result<(), u32> {
    let name: Vec<u16> = to.as_os_str().encode_wide().collect();
    let name_bytes = name.len() * 2;
    let size = (20 + name_bytes + 2).max(24);
    let mut buffer: Vec<u64> = vec![0; size.div_ceil(8)];
    let p = buffer.as_mut_ptr() as *mut u8;
    // SAFETY: the buffer is 8-byte aligned, zeroed and at least `size` bytes; every write stays inside it.
    unsafe {
        *(p as *mut u32) = 1 | 2;
        *(p.add(16) as *mut u32) = name_bytes as u32;
        std::ptr::copy_nonoverlapping(name.as_ptr() as *const u8, p.add(20), name_bytes);
        if SetFileInformationByHandle(file.as_raw_handle() as isize, 22, p as *const c_void, size as u32) == 0 {
            return Err(GetLastError());
        }
    }
    Ok(())
}

struct Rng(u64);

impl Rng {
    fn below(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % n.max(1)
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let holders = args.iter().any(|a| a == "holders");
    let seconds: u64 = args.iter().filter_map(|a| a.parse().ok()).next().unwrap_or(8);
    let scratch = Scratch::new("ao-rename-probe");
    let dir = fs::canonicalize(scratch.path()).expect("the scratch folder");
    let target = dir.join("essay.md");
    fs::write(&target, "start").expect("the target");
    let start = Instant::now();
    let stop = Arc::new(AtomicBool::new(false));
    // When each of the other program's files took the name, in microseconds, and its index.
    let placed: Arc<Mutex<Vec<(u128, u64)>>> = Arc::new(Mutex::new(Vec::new()));

    let other = {
        let (target, dir, stop, placed) = (target.clone(), dir.clone(), stop.clone(), placed.clone());
        std::thread::spawn(move || {
            let mut rng = Rng(0x1234_5678_9ABC_DEF1);
            let mut k = 0u32;
            while !stop.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(3 + rng.below(9)));
                k += 1;
                let temp = dir.join(format!("essay.md.other-{k}"));
                for _ in 0..400 {
                    if fs::write(&temp, format!("OUT{k}")).is_ok() {
                        let index = File::open(&temp).ok().and_then(|f| index_of(&f));
                        if fs::rename(&temp, &target).is_ok() {
                            if let Some(index) = index {
                                placed.lock().unwrap().push((start.elapsed().as_micros(), index));
                            }
                            break;
                        }
                    }
                    let _ = fs::remove_file(&temp);
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
            k
        })
    };

    let holding: Vec<_> = (0..if holders { 2u64 } else { 0 })
        .map(|which| {
            let (target, stop) = (target.clone(), stop.clone());
            std::thread::spawn(move || {
                let mut rng = Rng(0xA5A5_0000_1111_2222 + which * 7919);
                let accesses = [0x8000_0000u32, 0x8000_0000, 0xC000_0000, 0x0001_0000, 0x80, 0x4];
                while !stop.load(Ordering::Relaxed) {
                    let access = accesses[rng.below(accesses.len() as u64) as usize];
                    if let Ok(file) = OpenOptions::new().access_mode(access).share_mode(rng.below(8) as u32).open(&target) {
                        std::thread::sleep(Duration::from_millis(rng.below(4)));
                        drop(file);
                    }
                    std::thread::sleep(Duration::from_millis(1 + rng.below(12)));
                }
            })
        })
        .collect();

    // The saver, as the store saves.
    let mut rng = Rng(0x0BAD_5EED_0000_0001);
    let (mut saves, mut caught, mut refused) = (0u32, 0u32, 0u32);
    // For each save that went through: when the look began, when the rename returned, how long the rename call took, the index guarded.
    let mut done: Vec<(u128, u128, u128, u64)> = Vec::new();
    let mut n = 0u32;
    while start.elapsed() < Duration::from_secs(seconds) {
        std::thread::sleep(Duration::from_millis(2 + rng.below(8)));
        n += 1;
        let Ok(guard) = OpenOptions::new().read(true).share_mode(1 | 4).open(&target) else { continue };
        let Some(guarded) = index_of(&guard) else { continue };
        let temp = dir.join(format!("~essay.md.{n:08x}.tmp"));
        let Ok(mut file) = OpenOptions::new().write(true).create_new(true).access_mode(0x4000_0000 | 0x0001_0000 | 0x0010_0000).share_mode(1 | 4).open(&temp) else { continue };
        let _ = file.write_all(format!("MINE{n}").as_bytes());
        let _ = file.sync_all();
        let looked_at = start.elapsed().as_micros();
        if look(&target) != Some(guarded) {
            caught += 1;
            let yes: u8 = 1;
            // SAFETY: FILE_DISPOSITION_INFO is one BOOLEAN; the pointer is valid for the call.
            unsafe { SetFileInformationByHandle(file.as_raw_handle() as isize, 4, &yes as *const u8 as *const c_void, 1) };
            continue;
        }
        let before_rename = start.elapsed().as_micros();
        match rename_over(&file, &target) {
            Ok(()) => {
                let after = start.elapsed().as_micros();
                saves += 1;
                done.push((looked_at, after, after - before_rename, guarded));
                let _ = file.sync_all();
            }
            Err(_) => {
                refused += 1;
                let yes: u8 = 1;
                // SAFETY: as above.
                unsafe { SetFileInformationByHandle(file.as_raw_handle() as isize, 4, &yes as *const u8 as *const c_void, 1) };
            }
        }
        drop(guard);
    }
    stop.store(true, Ordering::Relaxed);
    let others = other.join().expect("the other program");
    for thread in holding {
        let _ = thread.join();
    }

    let placed = placed.lock().unwrap().clone();
    let mut unseen = Vec::new();
    for &(looked_at, after, rename_took, _) in &done {
        // One of the other program's files took the name after our look began and before our rename returned.
        if let Some(&(at, _)) = placed.iter().find(|&&(at, _)| at > looked_at && at < after) {
            unseen.push((at - looked_at, after - at, rename_took));
        }
    }
    let mut renames: Vec<u128> = done.iter().map(|d| d.2).collect();
    renames.sort();
    let pick = |q: f64| renames.get(((renames.len() as f64 - 1.0) * q) as usize).copied().unwrap_or(0);
    println!("holders: {holders}; {seconds} s; the other program placed {} files ({others} tries)", placed.len());
    println!("our saves: {saves} went through, {caught} stopped by the look, {refused} refused by the rename");
    println!("the rename call took: median {} us, 90% {} us, 99% {} us, longest {} us", pick(0.5), pick(0.9), pick(0.99), pick(1.0));
    println!("renames that replaced a file placed after the look: {}", unseen.len());
    for (after_look, before_return, rename_took) in unseen.iter().take(12) {
        println!("  placed {after_look} us after the look began, {before_return} us before our rename returned; that rename call took {rename_took} us");
    }
}
