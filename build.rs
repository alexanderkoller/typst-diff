use std::env;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");

    let unix = build_unix_timestamp();
    println!("cargo:rustc-env=TYPST_DIFF_BUILD_UNIX={unix}");
    println!(
        "cargo:rustc-env=TYPST_DIFF_BUILD_UTC={}",
        utc_timestamp(unix)
    );
}

fn build_unix_timestamp() -> u64 {
    if let Some(value) = env::var_os("SOURCE_DATE_EPOCH") {
        let value = value
            .to_str()
            .expect("SOURCE_DATE_EPOCH must contain valid UTF-8");
        return value
            .parse()
            .expect("SOURCE_DATE_EPOCH must be an unsigned Unix timestamp");
    }

    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is before the Unix epoch")
        .as_secs()
}

fn utc_timestamp(unix: u64) -> String {
    let days = (unix / 86_400) as i64;
    let seconds = unix % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = seconds / 3_600;
    let minute = (seconds % 3_600) / 60;
    let second = seconds % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn civil_from_days(days_since_epoch: i64) -> (i32, u32, u32) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if m <= 2 { 1 } else { 0 };
    (year as i32, m as u32, d as u32)
}
