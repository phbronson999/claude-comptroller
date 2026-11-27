//! "Are our seats worth it compared with paying API prices?" For a period, each person's
//! seat cost (their plan's price, spread evenly per day and counted from the day they first
//! connected) against what their recorded tokens would cost at API list prices.
//!
//! Claude Code traffic only: Chat and Cowork use the same seats but don't pass through.

use std::collections::BTreeMap;
use std::path::Path;

use rusqlite::OpenFlags;

use crate::activity;
use crate::config::{Cost as CostConfig, SeatPlan};
use crate::directory::open;
use crate::health::format_utc;
use crate::pricing::{Pricing, Tokens};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Period {
    /// The billing month so far, from `Cost.MonthStartDay`.
    Month,
    Last30Days,
    /// From the last renewal of the first annual plan with a renewal date.
    SeatYear,
}

// ── dates, as days since 1970 ───────────────────────────────────────────────

pub fn day_number(y: i64, m: u32, d: u32) -> i64 {
    let (m, d) = (m as i64, d as i64);
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468
}

pub fn civil(day: i64) -> (i64, u32, u32) {
    let z = day + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

fn days_in_month(y: i64, m: u32) -> u32 {
    let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
    (day_number(ny, nm, 1) - day_number(y, m, 1)) as u32
}

/// The day `d` of a month, or its last day when it's shorter (Feb 29 in other years).
fn clamped(y: i64, m: u32, d: u32) -> i64 {
    day_number(y, m, d.min(days_in_month(y, m)))
}

fn add_months(y: i64, m: u32, months: i64) -> (i64, u32) {
    let index = y * 12 + (m as i64 - 1) + months;
    (index.div_euclid(12), (index.rem_euclid(12) + 1) as u32)
}

/// The billing month that contains `day`: its first day and the first day of the next.
fn billing_month(day: i64, start_day: u32) -> (i64, i64) {
    let (y, m, d) = civil(day);
    let (sy, sm) = if d >= start_day { (y, m) } else { add_months(y, m, -1) };
    let (ny, nm) = add_months(sy, sm, 1);
    (clamped(sy, sm, start_day), clamped(ny, nm, start_day))
}

/// The seat year that contains `day`, from a renewal date: its first day and the next renewal.
fn seat_year(day: i64, renewal: (u32, u32)) -> (i64, i64) {
    let (y, _, _) = civil(day);
    let this = clamped(y, renewal.0, renewal.1);
    let start_year = if day >= this { y } else { y - 1 };
    (clamped(start_year, renewal.0, renewal.1), clamped(start_year + 1, renewal.0, renewal.1))
}

fn parse_date(text: &str) -> Option<(i64, u32, u32)> {
    let mut parts = text.trim().split('-');
    let (y, m, d) = (parts.next()?.parse().ok()?, parts.next()?.parse().ok()?, parts.next()?.parse().ok()?);
    ((1..=12).contains(&m) && (1..=31).contains(&d)).then_some((y, m, d))
}

const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

fn short(day: i64) -> String {
    let (_, m, d) = civil(day);
    format!("{} {d}", &MONTHS[m as usize - 1][..3])
}

// ── the report ──────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub struct Range {
    /// Local days, inclusive.
    pub first_day: i64,
    pub last_day: i64,
    /// The period's last day when it runs on past today, for projections.
    pub end_day: i64,
    pub label: String,
}

impl Range {
    pub fn days(&self) -> i64 {
        self.last_day - self.first_day + 1
    }
}

pub fn range(period: Period, today: i64, cost: &CostConfig) -> Result<Range, String> {
    match period {
        Period::Month => {
            let (first, next) = billing_month(today, cost.month_start_day.clamp(1, 28));
            let (_, m1, d1) = civil(first);
            let (_, m2, d2) = civil(today);
            let label = if m1 == m2 { format!("{} {d1} – {d2}", MONTHS[m1 as usize - 1]) } else { format!("{} – {}", short(first), short(today)) };
            Ok(Range { first_day: first, last_day: today, end_day: next - 1, label })
        }
        Period::Last30Days => Ok(Range { first_day: today - 29, last_day: today, end_day: today, label: format!("last 30 days · {} – {}", short(today - 29), short(today)) }),
        Period::SeatYear => {
            let renewal = renewal(cost).ok_or("Add a renewal date to an annual seat plan (s on this screen) to see the seat year.")?;
            let (first, next) = seat_year(today, renewal);
            let (y, _, _) = civil(first);
            Ok(Range { first_day: first, last_day: today, end_day: next - 1, label: format!("seat year from {}, {y}", short(first)) })
        }
    }
}

/// The renewal month and day of the first annual plan that has one.
fn renewal(cost: &CostConfig) -> Option<(u32, u32)> {
    cost.seat_plans.iter().filter(|p| p.billing == "annual").find_map(|p| p.renewal_date.as_deref().and_then(parse_date)).map(|(_, m, d)| (m, d))
}

/// A seat's cost from `from` to `to` (local days, inclusive), each day a share of the billing
/// period it falls in: a month for monthly plans, the seat year for annual ones.
pub fn seat_cost(plan: &SeatPlan, from: i64, to: i64, month_start_day: u32) -> Option<f64> {
    let price = plan.price?;
    let renewal = plan.renewal_date.as_deref().and_then(parse_date).map(|(_, m, d)| (m, d));
    let mut total = 0.0;
    let mut day = from;
    while day <= to {
        let (start, next) = if plan.billing == "monthly" {
            billing_month(day, month_start_day.clamp(1, 28))
        } else if let Some(renewal) = renewal {
            seat_year(day, renewal)
        } else {
            // No renewal date: an even share of a 365-day year.
            (day, day + 365)
        };
        let last = (next - 1).min(to);
        total += price * (last - day + 1) as f64 / (next - start) as f64;
        day = last + 1;
    }
    Some(total)
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PersonCost {
    pub name: String,
    pub plan: Option<String>,
    /// None: no plan, or the plan has no price.
    pub seat: Option<f64>,
    pub api: f64,
    pub tokens: Tokens,
    /// Tokens on models not in the pricing table, so not in `api`.
    pub unpriced_tokens: i64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    pub range: Option<Range>,
    pub people: Vec<PersonCost>,
    /// Pricing-table model name, API cost and tokens; most expensive first.
    pub models: Vec<(String, f64, Tokens)>,
    /// Model ids not in the pricing table, with their tokens.
    pub unpriced: Vec<(String, i64)>,
    pub tokens: Tokens,
    pub api: f64,
    pub seat: f64,
    /// People with a seat cost counted.
    pub seats: usize,
    /// Plan names without a price.
    pub plans_without_price: Vec<String>,
    pub people_without_plan: usize,
    /// For the seat year: the whole year's seat cost, and API cost at today's daily rate.
    pub projection: Option<(f64, f64)>,
}

impl Report {
    /// How many times cheaper the seats are than the API (above 1), or dearer (below 1).
    pub fn ratio(&self) -> Option<f64> {
        (self.seat > 0.0 && self.api > 0.0).then(|| self.api / self.seat)
    }
}

/// Builds the report for `period`, `today` being a local day number and `offset` local minus UTC.
pub fn report(db: &Path, cost: &CostConfig, pricing: &Pricing, period: Period, today: i64, offset: i64) -> Result<Report, String> {
    let range = range(period, today, cost)?;
    let from_utc = format_utc(range.first_day * 86_400 - offset);
    activity::track(
        format!("sqlite3 \"{}\" \"SELECT user_name, model, …tokens FROM requests WHERE usage = 'parsed' AND started_utc >= '{from_utc}'\"", db.display()),
        || build(db, cost, pricing, range.clone(), &from_utc, offset),
        |r| format!("{} people · {} tokens", r.people.len(), r.tokens.total()),
    )
}

fn build(db: &Path, cost: &CostConfig, pricing: &Pricing, range: Range, from_utc: &str, offset: i64) -> Result<Report, String> {
    let c = open(db, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let e = |e: rusqlite::Error| e.to_string();
    let mut people: BTreeMap<String, PersonCost> = BTreeMap::new();
    let mut first_days: BTreeMap<String, i64> = BTreeMap::new();

    let users = c
        .prepare("SELECT name, created_utc, seat_plan FROM users")
        .map_err(e)?
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?)))
        .map_err(e)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(e)?;
    for (name, created, plan) in users {
        let first = crate::health::parse_utc(&created).map(|t| (t + offset).div_euclid(86_400)).unwrap_or(range.first_day);
        first_days.insert(name.clone(), first);
        people.insert(name.clone(), PersonCost { name, plan, ..Default::default() });
    }

    let mut report = Report::default();
    let mut models: BTreeMap<String, (f64, Tokens)> = BTreeMap::new();
    let mut unpriced: BTreeMap<String, i64> = BTreeMap::new();
    let mut statement = c
        .prepare(
            "SELECT COALESCE(user_name, ''), COALESCE(model, ''), COALESCE(input_tokens, 0), COALESCE(output_tokens, 0),
                    COALESCE(cache_write_tokens, 0), cache_write_5m_tokens, cache_write_1h_tokens, COALESCE(cache_read_tokens, 0)
             FROM requests WHERE usage = 'parsed' AND started_utc >= ?1",
        )
        .map_err(e)?;
    let rows = statement
        .query_map([from_utc], |r| {
            let write: i64 = r.get(4)?;
            let (w5, w1): (Option<i64>, Option<i64>) = (r.get(5)?, r.get(6)?);
            // Without the split, every cache write is priced as a 5-minute one.
            let (cache_write_5m, cache_write_1h) = match (w5, w1) {
                (None, None) => (write, 0),
                (a, b) => (a.unwrap_or(0), b.unwrap_or(0)),
            };
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                Tokens { input: r.get(2)?, output: r.get(3)?, cache_write_5m, cache_write_1h, cache_read: r.get(7)? },
            ))
        })
        .map_err(e)?;
    for row in rows {
        let (user, model, tokens) = row.map_err(e)?;
        let person = people.entry(user.clone()).or_insert_with(|| PersonCost { name: user.clone(), ..Default::default() });
        person.tokens.add(&tokens);
        report.tokens.add(&tokens);
        match (pricing.cost(&model, &tokens), pricing.model(&model)) {
            (Some(price), Some(entry)) => {
                person.api += price;
                report.api += price;
                let slot = models.entry(entry.name.clone()).or_default();
                slot.0 += price;
                slot.1.add(&tokens);
            }
            _ => {
                person.unpriced_tokens += tokens.total();
                *unpriced.entry(if model.is_empty() { "unknown".into() } else { model }).or_default() += tokens.total();
            }
        }
    }

    // Seats
    let month_start = cost.month_start_day;
    for person in people.values_mut() {
        let Some(name) = &person.plan else {
            report.people_without_plan += 1;
            continue;
        };
        let Some(plan) = cost.seat_plans.iter().find(|p| &p.name == name) else { continue };
        if plan.price.is_none() {
            if !report.plans_without_price.contains(name) {
                report.plans_without_price.push(name.clone());
            }
            continue;
        }
        let from = range.first_day.max(*first_days.get(&person.name).unwrap_or(&range.first_day));
        if from > range.last_day {
            person.seat = Some(0.0);
            continue;
        }
        person.seat = seat_cost(plan, from, range.last_day, month_start);
        report.seat += person.seat.unwrap_or(0.0);
        report.seats += 1;
    }

    if range.end_day > range.last_day {
        let year_seats: f64 = people
            .values()
            .filter_map(|p| {
                let plan = cost.seat_plans.iter().find(|s| Some(&s.name) == p.plan.as_ref())?;
                let from = range.first_day.max(*first_days.get(&p.name)?);
                seat_cost(plan, from, range.end_day, month_start)
            })
            .sum();
        let projected = report.api / range.days() as f64 * (range.end_day - range.first_day + 1) as f64;
        report.projection = Some((year_seats, projected));
    }

    report.people = people.into_values().filter(|p| p.plan.is_some() || p.tokens.total() > 0).collect();
    report.people.sort_by(|a, b| b.api.total_cmp(&a.api).then_with(|| a.name.cmp(&b.name)));
    report.models = models.into_iter().map(|(name, (cost, tokens))| (name, cost, tokens)).collect();
    report.models.sort_by(|a, b| b.1.total_cmp(&a.1));
    report.unpriced = unpriced.into_iter().collect();
    report.range = Some(range);
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn plan(billing: &str, price: f64, renewal: Option<&str>) -> SeatPlan {
        SeatPlan { name: "P".into(), billing: billing.into(), price: Some(price), renewal_date: renewal.map(String::from) }
    }

    #[test]
    fn calendar_round_trips() {
        for day in [-1000, 0, 11_016, 20_000, 20_454] {
            let (y, m, d) = civil(day);
            assert_eq!(day_number(y, m, d), day);
        }
        assert_eq!(civil(day_number(2028, 2, 29)), (2028, 2, 29));
        assert_eq!(days_in_month(2027, 2), 28);
        assert_eq!(clamped(2027, 2, 31), day_number(2027, 2, 28));
    }

    #[test]
    fn a_whole_period_costs_the_plan_price() {
        // A monthly plan over one billing month, and an annual plan over one seat year.
        let monthly = plan("monthly", 100.0, None);
        assert!((seat_cost(&monthly, day_number(2026, 2, 1), day_number(2026, 2, 28), 1).unwrap() - 100.0).abs() < 1e-9);
        assert!((seat_cost(&monthly, day_number(2026, 2, 15), day_number(2026, 3, 14), 15).unwrap() - 100.0).abs() < 1e-9);
        let annual = plan("annual", 300.0, Some("2025-03-01"));
        assert!((seat_cost(&annual, day_number(2026, 3, 1), day_number(2027, 2, 28), 1).unwrap() - 300.0).abs() < 1e-9);
        // Half of a 30-day month.
        assert!((seat_cost(&monthly, day_number(2026, 11, 1), day_number(2026, 11, 15), 1).unwrap() - 50.0).abs() < 1e-9);
    }

    #[test]
    fn periods() {
        let mut cost = CostConfig::default();
        let today = day_number(2026, 11, 27);
        let month = range(Period::Month, today, &cost).unwrap();
        assert_eq!((month.first_day, month.end_day, month.label.as_str()), (day_number(2026, 11, 1), day_number(2026, 11, 30), "November 1 – 27"));
        cost.month_start_day = 28;
        assert_eq!(range(Period::Month, today, &cost).unwrap().first_day, day_number(2026, 10, 28));
        assert_eq!(range(Period::Last30Days, today, &cost).unwrap().days(), 30);
        assert!(range(Period::SeatYear, today, &cost).is_err(), "no renewal date yet");
        cost.seat_plans[0].renewal_date = Some("2024-03-15".into());
        let year = range(Period::SeatYear, today, &cost).unwrap();
        assert_eq!((year.first_day, year.end_day), (day_number(2026, 3, 15), day_number(2027, 3, 14)));
    }

    #[test]
    fn report_compares_seats_with_api_prices() {
        let db = std::env::temp_dir().join(format!("comptroller-cost-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&db);
        let c = Connection::open(&db).unwrap();
        c.execute_batch(include_str!("../../contracts/schema.sql")).unwrap();
        c.execute("INSERT INTO schema_version (version) VALUES (?1)", [crate::SCHEMA_VERSION as i64]).unwrap();
        let now = crate::health::now();
        let today = now.div_euclid(86_400);
        c.execute_batch(&format!(
            "INSERT INTO users (name, source, created_utc, seat_plan) VALUES
               ('amy', 'manual', '2020-01-01T00:00:00.000Z', 'Max'), ('bo', 'manual', '2020-01-01T00:00:00.000Z', NULL);
             INSERT INTO requests (started_utc, user_name, method, path, status, duration_ms, response_bytes, model, input_tokens, output_tokens, cache_write_tokens, cache_read_tokens, usage) VALUES
               ('{t}', 'amy', 'POST', '/v1/messages', 200, 1, 1, 'claude-opus-5-5', 1000000, 1000000, 0, 0, 'parsed'),
               ('{t}', 'bo', 'POST', '/v1/messages', 200, 1, 1, 'claude-mock-1', 500, 500, 0, 0, 'parsed'),
               ('{t}', 'bo', 'POST', '/v1/messages', 429, 1, 1, NULL, NULL, NULL, NULL, NULL, NULL);",
            t = format_utc(now)
        ))
        .unwrap();
        drop(c);

        let mut cost = CostConfig::default();
        cost.seat_plans = vec![SeatPlan { name: "Max".into(), billing: "monthly".into(), price: Some(100.0), renewal_date: None }];
        let pricing = Pricing::parse(crate::pricing::BUILT_IN).unwrap();
        let report = report(&db, &cost, &pricing, Period::Last30Days, today, 0).unwrap();

        let opus = pricing.model("claude-opus-5-5").unwrap();
        assert!((report.api - (opus.rates.input + opus.rates.output)).abs() < 1e-9);
        assert_eq!(report.people[0].name, "amy");
        assert!(report.people[0].seat.unwrap() > 0.0);
        assert_eq!(report.seats, 1);
        assert_eq!(report.people_without_plan, 1);
        assert_eq!(report.unpriced, vec![("claude-mock-1".to_string(), 1000)]);
        assert_eq!(report.models.len(), 1);
        assert_eq!(report.tokens.total(), 2_001_000);
        assert!(report.ratio().is_some());
    }
}
