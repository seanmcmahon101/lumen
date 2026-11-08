//! Offline unit and calendar calculations; no network or Office process required.
pub struct Answer {
    pub display: String,
    pub copy: String,
    pub note: String,
}

pub fn units(input: &str) -> Option<Answer> {
    let words: Vec<_> = input.split_whitespace().collect();
    if words.len() != 4 || !matches!(words[2].to_ascii_lowercase().as_str(), "in" | "to") {
        return None;
    }
    let value: f64 = words[0].parse().ok()?;
    let (kind, scale, offset) = unit(words[1])?;
    let (other, target_scale, target_offset) = unit(words[3])?;
    if kind != other || !value.is_finite() {
        return None;
    }
    let result = (value * scale + offset - target_offset) / target_scale;
    if !result.is_finite() {
        return None;
    }
    let (display, raw) = crate::calc::format(result);
    Some(Answer {
        display: format!("{display} {}", words[3]),
        copy: raw,
        note: format!("{input} · Enter copies the number"),
    })
}

fn unit(name: &str) -> Option<(u8, f64, f64)> {
    let name = name.to_lowercase();
    let (dimension, scale) = match name.as_str() {
        "m" | "metre" | "metres" | "meter" | "meters" => (0, 1.),
        "km" | "kilometres" | "kilometers" => (0, 1000.),
        "cm" => (0, 0.01),
        "mm" => (0, 0.001),
        "mi" | "mile" | "miles" => (0, 1609.344),
        "ft" | "foot" | "feet" => (0, 0.3048),
        "in" | "inch" | "inches" => (0, 0.0254),
        "yd" | "yards" => (0, 0.9144),
        "m2" | "m²" => (1, 1.),
        "km2" | "km²" => (1, 1e6),
        "ft2" | "ft²" => (1, 0.09290304),
        "acre" | "acres" => (1, 4046.8564224),
        "ha" | "hectares" => (1, 10000.),
        "kg" | "kilograms" => (2, 1.),
        "g" | "grams" => (2, 0.001),
        "mg" => (2, 1e-6),
        "lb" | "lbs" | "pounds" => (2, 0.45359237),
        "oz" | "ounces" => (2, 0.028349523125),
        "stone" | "st" => (2, 6.35029318),
        "l" | "litres" | "liters" => (3, 1.),
        "ml" => (3, 0.001),
        "ukgal" => (3, 4.54609),
        "usgal" => (3, 3.785411784),
        "s" | "sec" | "seconds" => (4, 1.),
        "ms" => (4, 0.001),
        "min" | "minutes" => (4, 60.),
        "h" | "hr" | "hours" => (4, 3600.),
        "d" | "days" => (4, 86400.),
        "weeks" | "wk" => (4, 604800.),
        "b" | "bytes" => (5, 1.),
        "kb" => (5, 1e3),
        "mb" => (5, 1e6),
        "gb" => (5, 1e9),
        "tb" => (5, 1e12),
        "kib" => (5, 1024.),
        "mib" => (5, 1048576.),
        "gib" => (5, 1073741824.),
        "tib" => (5, 1099511627776.),
        "c" | "°c" | "celsius" => return Some((6, 1., 273.15)),
        "f" | "°f" | "fahrenheit" => return Some((6, 5. / 9., 273.15 - 32. * 5. / 9.)),
        "k" | "kelvin" => return Some((6, 1., 0.)),
        _ => return None,
    };
    Some((dimension, scale, 0.))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Date {
    year: i64,
    month: i64,
    day: i64,
}
impl Date {
    fn today() -> Self {
        let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
        Self {
            year: t.wYear as i64,
            month: t.wMonth as i64,
            day: t.wDay as i64,
        }
    }
    fn parse(s: &str, today: Self) -> Option<Self> {
        if s.eq_ignore_ascii_case("today") {
            return Some(today);
        }
        let p: Vec<_> = s.split('-').collect();
        if p.len() != 3 || p[0].len() != 4 || p[1].len() != 2 || p[2].len() != 2 {
            return None;
        }
        let d = Self {
            year: p[0].parse().ok()?,
            month: p[1].parse().ok()?,
            day: p[2].parse().ok()?,
        };
        if !(1..=9999).contains(&d.year)
            || !(1..=12).contains(&d.month)
            || !(1..=31).contains(&d.day)
        {
            return None;
        }
        (Self::from_days(d.days()) == d).then_some(d)
    }
    // Gregorian 400-year cycles; these operate on calendar days rather than hours across DST.
    fn days(self) -> i64 {
        let year = self.year - i64::from(self.month <= 2);
        let era = year.div_euclid(400);
        let y = year - era * 400;
        let m = self.month + if self.month > 2 { -3 } else { 9 };
        era * 146097 + y * 365 + y / 4 - y / 100 + (153 * m + 2) / 5 + self.day - 1
    }
    fn from_days(days: i64) -> Self {
        let era = days.div_euclid(146097);
        let d = days - era * 146097;
        let y = (d - d / 1460 + d / 36524 - d / 146096) / 365;
        let day = d - (365 * y + y / 4 - y / 100);
        let m = (5 * day + 2) / 153;
        let month = m + if m < 10 { 3 } else { -9 };
        Self {
            year: y + era * 400 + i64::from(month <= 2),
            month,
            day: day - (153 * m + 2) / 5 + 1,
        }
    }
    fn text(self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

pub fn date(input: &str) -> Option<Answer> {
    date_at(input, Date::today())
}
fn date_at(input: &str, today: Date) -> Option<Answer> {
    let input = input.trim();
    let input = if input.is_empty() { "today" } else { input };
    let words: Vec<_> = input.split_whitespace().collect();
    let start = Date::parse(words[0], today)?;
    if words.len() == 1 {
        let text = start.text();
        return Some(Answer {
            display: text.clone(),
            copy: text,
            note: "Local calendar date · Enter copies".into(),
        });
    }
    if !matches!(words.get(1).copied(), Some("+" | "-")) {
        return None;
    }
    if words.len() == 3
        && words[1] == "-"
        && let Some(end) = Date::parse(words[2], today)
    {
        let days = start.days() - end.days();
        return Some(Answer {
            display: format!("{days} days"),
            copy: days.to_string(),
            note: input.into(),
        });
    }
    if !(3..=4).contains(&words.len()) {
        return None;
    }
    let amount: i64 = words[2].parse().ok()?;
    let scale = match words.get(3).map(|s| s.to_ascii_lowercase()).as_deref() {
        None | Some("day" | "days" | "d") => 1,
        Some("week" | "weeks" | "wk") => 7,
        _ => return None,
    };
    let offset = amount
        .checked_mul(scale)?
        .checked_mul(if words[1] == "-" { -1 } else { 1 })?;
    let days = start.days().checked_add(offset)?;
    if !(Date {
        year: 1,
        month: 1,
        day: 1,
    }
    .days()..=Date {
        year: 9999,
        month: 12,
        day: 31,
    }
    .days())
        .contains(&days)
    {
        return None;
    }
    let text = Date::from_days(days).text();
    Some(Answer {
        display: text.clone(),
        copy: text,
        note: input.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn conversions_check_dimensions_and_offsets() {
        assert_eq!(units("10 miles in km").unwrap().copy, "16.09344");
        assert_eq!(units("2 GB in MB").unwrap().copy, "2000");
        assert_eq!(units("2 GiB in MiB").unwrap().copy, "2048");
        assert_eq!(units("32 F in C").unwrap().copy, "0");
        assert_eq!(units("-40 C to F").unwrap().copy, "-40");
        assert!(units("10 kg in km").is_none());
        assert!(units("NaN m in ft").is_none());
    }
    #[test]
    fn calendar_handles_leap_days_year_boundaries_and_bad_dates() {
        let today = Date {
            year: 2024,
            month: 2,
            day: 28,
        };
        let answer = |s| date_at(s, today).map(|a| a.copy);
        assert_eq!(answer("today + 1 day").as_deref(), Some("2024-02-29"));
        assert_eq!(answer("today + 2 days").as_deref(), Some("2024-03-01"));
        assert_eq!(answer("2026-01-01 - 1 day").as_deref(), Some("2025-12-31"));
        assert_eq!(answer("2024-03-01 - today").as_deref(), Some("2"));
        assert_eq!(answer("today + 2 weeks").as_deref(), Some("2024-03-13"));
        for bad in [
            "2023-02-29",
            "2024-13-01",
            "2024-04-31",
            "today + 999999999999999999 days",
            "9999-12-31 + 1",
        ] {
            assert!(answer(bad).is_none(), "{bad}");
        }
    }
}
