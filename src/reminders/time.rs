use super::Draft;
use anyhow::{Context, Result, bail, ensure};
use chrono::{DateTime, Duration, Local, LocalResult, NaiveDate, NaiveTime, TimeZone};
use regex::Regex;
use std::sync::LazyLock;

static RELATIVE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^(?:in\s*)?(\d+)\s*(m|min|mins|minutes?|分钟|分|h|hours?|小时|d|days?|天)(?:后)?$",
    )
    .unwrap()
});
static CLOCK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:(今天|明天|后天|today|tomorrow|\d{4}-\d{2}-\d{2})\s*)?(?:(上午|下午|晚上|早上|早|中午)\s*)?(\d{1,2})(?::(\d{2})|点(半|\d{1,2}分?)?)$").unwrap()
});
static QUICK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
    r"(?i)^((?:in\s*)?\d+\s*(?:minutes?|mins?|hours?|days?|分钟|小时|分|天|m|h|d)(?:后)?",
    r"|(?:(?:今天|明天|后天|today|tomorrow|\d{4}-\d{2}-\d{2})\s*)?(?:(?:上午|下午|晚上|早上|早|中午)\s*)?\d{1,2}(?::\d{2}|点(?:半|\d{1,2}分?)?))\s+(.+)$"
)).unwrap()
});

pub fn resolve(input: &str, now: DateTime<Local>) -> Result<i64> {
    resolve_in_zone(input, now)
}

fn resolve_in_zone<T: TimeZone>(input: &str, now: DateTime<T>) -> Result<i64> {
    let input = input.trim().to_lowercase();
    let input = input
        .replace("明早", "明天早上")
        .replace("今晚", "今天晚上");
    if let Some(caps) = RELATIVE.captures(&input) {
        let amount: i64 = caps[1].parse().context("时间数值过大")?;
        let unit = &caps[2];
        let factor = if matches!(unit, "h" | "hour" | "hours" | "小时") {
            3600
        } else if matches!(unit, "d" | "day" | "days" | "天") {
            86400
        } else {
            60
        };
        let seconds = amount.checked_mul(factor).context("时间数值过大")?;
        ensure!(
            (1..=366 * 86400).contains(&seconds),
            "请设置 1 分钟到 366 天以内的提醒"
        );
        return Ok((now + Duration::seconds(seconds)).timestamp());
    }
    let caps = CLOCK
        .captures(&input)
        .context("请输入 10m、15:00、明天 9:00 或 2026-10-20 15:00")?;
    let mut hour: u32 = caps[3].parse()?;
    let minutes = caps
        .get(4)
        .map(|v| v.as_str())
        .or_else(|| caps.get(5).map(|v| v.as_str()))
        .unwrap_or("0");
    let minute: u32 = if minutes == "半" {
        30
    } else {
        minutes.trim_end_matches('分').parse()?
    };
    if let Some(period) = caps.get(2) {
        ensure!((1..=12).contains(&hour), "上午、下午的小时应为 1–12");
        if matches!(period.as_str(), "下午" | "晚上" | "中午") {
            if hour != 12 {
                hour += 12;
            }
        } else if hour == 12 {
            hour = 0;
        }
    }
    let time = NaiveTime::from_hms_opt(hour, minute, 0).context("时间无效，请检查小时和分钟")?;
    let explicit_day = caps.get(1).map(|v| v.as_str());
    let date = match explicit_day {
        None | Some("今天" | "today") => now.date_naive(),
        Some("明天" | "tomorrow") => now.date_naive() + Duration::days(1),
        Some("后天") => now.date_naive() + Duration::days(2),
        Some(date) => NaiveDate::parse_from_str(date, "%Y-%m-%d").context("日期无效")?,
    };
    let mut date = date;
    let due = loop {
        let due = match now.timezone().from_local_datetime(&date.and_time(time)) {
            LocalResult::Single(due) => due,
            LocalResult::Ambiguous(_, _) => {
                bail!("这个时间在夏令时切换中出现两次，请改用相对时间（例如 60m）")
            }
            LocalResult::None => bail!("这个本地时间不存在，请选择其他时间"),
        };
        if explicit_day.is_none() && due <= now {
            date += Duration::days(1);
            continue;
        }
        break due;
    };
    ensure!(due > now, "这个时间已经过去，请选择未来时间");
    ensure!(
        due.timestamp() - now.timestamp() <= 366 * 86400,
        "请选择 366 天以内的提醒"
    );
    Ok(due.timestamp())
}

pub fn quick(input: &str, now: DateTime<Local>) -> Result<Draft> {
    let normalized = input
        .trim()
        .replace("明早", "明天早上")
        .replace("今晚", "今天晚上");
    let caps = QUICK
        .captures(&normalized)
        .context("先写时间，再写事项，例如：10m 开会 / 明天 15:00 开会")?;
    Draft::new(caps[2].trim(), resolve(&caps[1], now)?)
}

pub fn display(timestamp: i64) -> String {
    Local
        .timestamp_opt(timestamp, 0)
        .single()
        .map(|date| date.format("%Y年%m月%d日 %H:%M").to_string())
        .unwrap_or_else(|| "时间无效".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{FixedOffset, Timelike};
    fn now() -> DateTime<FixedOffset> {
        FixedOffset::east_opt(8 * 3600)
            .unwrap()
            .with_ymd_and_hms(2026, 10, 8, 16, 0, 0)
            .unwrap()
    }
    #[test]
    fn relative_and_local_dates_have_predictable_meanings() {
        let now = now();
        assert_eq!(
            resolve_in_zone("10分钟后", now).unwrap(),
            (now + Duration::minutes(10)).timestamp()
        );
        assert_eq!(
            resolve_in_zone("in 2h", now).unwrap(),
            (now + Duration::hours(2)).timestamp()
        );
        let tomorrow = now + Duration::days(1);
        assert_eq!(
            resolve_in_zone("15:00", now).unwrap(),
            tomorrow.with_hour(15).unwrap().timestamp()
        );
        assert_eq!(
            resolve_in_zone("明天下午3点半", now).unwrap(),
            tomorrow
                .with_hour(15)
                .unwrap()
                .with_minute(30)
                .unwrap()
                .timestamp()
        );
        assert_eq!(
            resolve_in_zone("明早9点", now).unwrap(),
            tomorrow.with_hour(9).unwrap().timestamp()
        );
        assert_eq!(
            resolve_in_zone("2026-10-09 09:00", now).unwrap(),
            tomorrow.with_hour(9).unwrap().timestamp()
        );
    }
    #[test]
    fn bad_or_past_times_never_silently_roll_forward() {
        for input in [
            "今天 15:00",
            "0m",
            "999999999999999999999h",
            "3661d",
            "25:00",
            "明天 9:70",
            "2026-02-30 15:00",
            "下午15点",
            "一会儿",
            "15:00junk",
        ] {
            assert!(resolve_in_zone(input, now()).is_err(), "{input}");
        }
    }
    #[test]
    fn quick_input_requires_both_time_and_title() {
        let now = Local::now();
        let draft = quick("10m 开会", now).unwrap();
        assert_eq!(draft.title, "开会");
        assert_eq!(draft.due_at, (now + Duration::minutes(10)).timestamp());
        assert!(quick("10m", now).is_err());
        assert!(quick("10m   ", now).is_err());
        assert!(quick("开会 10m", now).is_err());
    }
}
