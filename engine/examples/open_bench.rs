//! Opening a big file, phase by phase, cold or warm, with page-fault counts:
//! `cargo run --release -p csvfab-engine --example open_bench -- FILE [MODE] [cold]`.
//! MODE: `open` (Base::open), `head` (Base::open_head), and raw strategies to
//! bring the bytes in: `touch`, `willneed`, `populate`, `pread:<MB>`, `read`.
//! `cold` evicts the file from the page cache first.

use csvfab_engine::{Base, OpenOptions};
use rayon::prelude::*;
use std::fs::File;
use std::os::fd::AsRawFd;
use std::time::Instant;

fn faults() -> (i64, i64) {
    // SAFETY: plain syscall into a zeroed struct.
    unsafe {
        let mut u: libc::rusage = std::mem::zeroed();
        libc::getrusage(libc::RUSAGE_SELF, &mut u);
        (u.ru_minflt, u.ru_majflt)
    }
}

fn evict(path: &str) {
    let f = File::open(path).unwrap();
    // SAFETY: valid fd.
    unsafe { libc::posix_fadvise(f.as_raw_fd(), 0, 0, libc::POSIX_FADV_DONTNEED) };
}

/// Sums one byte per page in parallel: what a scan pass costs in page faults alone.
fn touch(b: &[u8]) -> u64 {
    b.par_chunks(1 << 20).map(|c| c.iter().step_by(4096).map(|&x| x as u64).sum::<u64>()).sum()
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("file");
    let mode = args.next().unwrap_or_else(|| "open".into());
    let cold = args.next().is_some_and(|a| a == "cold");
    if cold { evict(&path); }
    let (f0, m0) = faults();
    let t = Instant::now();
    let what = match mode.as_str() {
        "open" => {
            let b = Base::open(path.as_ref(), OpenOptions::default(), None).unwrap();
            let s = format!("{} records, enc {:?}, index {} MB", b.n, b.enc, b.heap_bytes() >> 20);
            std::mem::forget(b);
            s
        }
        "phases" => {
            let f = File::open(&path).unwrap();
            // SAFETY: read-only map.
            let m = unsafe { memmap2::Mmap::map(&f).unwrap() };
            let _ = m.advise(memmap2::Advice::Sequential);
            let t1 = Instant::now();
            let ok = csvfab_engine::enc::valid_utf8(&m);
            let t2 = Instant::now();
            let (d, _) = csvfab_engine::base::detect_dialect(&m, None);
            let t3 = Instant::now();
            let s = csvfab_engine::scan::scan(&m, d, None);
            let t4 = Instant::now();
            let s2 = csvfab_engine::scan::scan_checked(&m, d, None, true);
            let t5 = Instant::now();
            format!("utf8 {ok} {:.1} ms, dialect {:.1} ms, scan {:.1} ms ({} records), scan+utf8 {:.1} ms {}", (t2 - t1).as_secs_f64() * 1e3, (t3 - t2).as_secs_f64() * 1e3, (t4 - t3).as_secs_f64() * 1e3, s.n, (t5 - t4).as_secs_f64() * 1e3, s2.utf8)
        }
        "head" => {
            let b = Base::open_head(path.as_ref(), OpenOptions::default()).unwrap();
            format!("{} records (head)", b.n)
        }
        "touch" | "willneed" | "populate" | "huge" => {
            let f = File::open(&path).unwrap();
            let mut o = memmap2::MmapOptions::new();
            if mode == "populate" { o.populate(); }
            // SAFETY: read-only map of a file nobody truncates during the bench.
            let m = unsafe { o.map(&f).unwrap() };
            if mode == "willneed" { m.advise(memmap2::Advice::WillNeed).unwrap(); }
            if mode == "huge" { let _ = m.advise(memmap2::Advice::HugePage); }
            format!("sum {}", touch(&m))
        }
        "popread" | "pophuge" => {
            let f = File::open(&path).unwrap();
            // SAFETY: read-only map of a file nobody truncates during the bench.
            let m = unsafe { memmap2::Mmap::map(&f).unwrap() };
            if mode == "pophuge" { let _ = m.advise(memmap2::Advice::HugePage); }
            let base = m.as_ptr() as usize;
            let len = m.len();
            (0..len.div_ceil(1 << 20)).into_par_iter().for_each(|k| {
                let a = k << 20;
                // SAFETY: a range of the mapping.
                unsafe { libc::madvise((base + a) as *mut libc::c_void, (len - a).min(1 << 20), 22) };
            });
            format!("sum {}", touch(&m))
        }
        "read" => {
            let v = std::fs::read(&path).unwrap();
            format!("{} bytes", v.len())
        }
        m if m.starts_with("pread:") => {
            let chunk: usize = m[6..].parse::<usize>().unwrap() << 20;
            let f = File::open(&path).unwrap();
            let len = f.metadata().unwrap().len() as usize;
            let mut v = vec![0u8; len];
            let fd = f.as_raw_fd();
            v.par_chunks_mut(chunk).enumerate().for_each(|(k, c)| {
                let mut done = 0;
                while done < c.len() {
                    // SAFETY: writes into c's own memory.
                    let r = unsafe { libc::pread(fd, c[done..].as_mut_ptr().cast(), c.len() - done, (k * chunk + done) as i64) };
                    assert!(r > 0);
                    done += r as usize;
                }
            });
            format!("{} bytes", v.len())
        }
        _ => panic!("mode?"),
    };
    let el = t.elapsed();
    let (f1, m1) = faults();
    let anon = std::fs::read_to_string("/proc/self/smaps_rollup").unwrap_or_default().lines().find(|l| l.starts_with("Anonymous")).unwrap_or("").split_whitespace().nth(1).unwrap_or("0").parse::<u64>().unwrap_or(0) >> 10;
    println!("anon {anon} MB");
    println!("{mode:>9} {} {:7.1} ms  minflt {:>7} majflt {:>6}  {what}", if cold { "cold" } else { "warm" }, el.as_secs_f64() * 1e3, f1 - f0, m1 - m0);
}
