//! Local calendar date as YYYYMMDD, using the OS directly (as the Zig build did through libc).
//!
//! Deliberately avoids crates that pull in platform frameworks: chrono's `clock` feature links
//! CoreFoundation on macOS, which breaks cross-compiling from Linux without an Apple SDK.

#[cfg(unix)]
pub fn today() -> Option<i32> {
    // SAFETY: `time` accepts a null pointer; `localtime_r` writes only into the `tm` we own and
    // is thread-safe, unlike `localtime`.
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut local: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&now, &mut local).is_null() {
            return None;
        }
        Some((local.tm_year + 1900) * 10000 + (local.tm_mon + 1) * 100 + local.tm_mday)
    }
}

#[cfg(windows)]
pub fn today() -> Option<i32> {
    #[repr(C)]
    #[derive(Default)]
    struct SystemTime {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        milliseconds: u16,
    }
    unsafe extern "system" {
        fn GetLocalTime(system_time: *mut SystemTime);
    }
    let mut now = SystemTime::default();
    // SAFETY: SYSTEMTIME has this exact layout and GetLocalTime only fills the struct.
    unsafe { GetLocalTime(&mut now) };
    Some(now.year as i32 * 10000 + now.month as i32 * 100 + now.day as i32)
}

#[cfg(test)]
mod tests {
    #[test]
    fn today_is_a_plausible_yyyymmdd() {
        let date = super::today().expect("clock available");
        let (year, month, day) = (date / 10000, date / 100 % 100, date % 100);
        assert!(
            (2024..2200).contains(&year) && (1..=12).contains(&month) && (1..=31).contains(&day),
            "{date}"
        );
    }
}
