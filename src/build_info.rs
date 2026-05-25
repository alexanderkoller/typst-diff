use typst::foundations::Datetime;
use typst_pdf::Timestamp;

pub const BUILD_UNIX: &str = env!("TYPST_DIFF_BUILD_UNIX");
pub const BUILD_UTC: &str = env!("TYPST_DIFF_BUILD_UTC");

pub fn build_report_line() -> String {
    format!(
        "typst-diff {} built {}",
        env!("CARGO_PKG_VERSION"),
        BUILD_UTC
    )
}

pub fn pdf_timestamp() -> Option<Timestamp> {
    let (date, time) = BUILD_UTC.strip_suffix('Z')?.split_once('T')?;
    let mut date_parts = date.split('-');u
    let year = date_parts.next()?.parse().ok()?;
    let month = date_parts.next()?.parse().ok()?;
    let day = date_parts.next()?.parse().ok()?;
    let mut time_parts = time.split(':');
    let hour = time_parts.next()?.parse().ok()?;
    let minute = time_parts.next()?.parse().ok()?;
    let second = time_parts.next()?.parse().ok()?;
    let datetime = Datetime::from_ymd_hms(year, month, day, hour, minute, second)?;
    Some(Timestamp::new_utc(datetime))
}
