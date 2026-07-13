//! The pure "who is on call now" resolver — no I/O, no repos. See `docs/oncall.md` for the
//! timezone/DST design rationale this module implements.

use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime, Utc, Weekday};
use chrono_tz::Tz;

use crate::oncall::model::{OnCallOverride, RotationKind};
use crate::user::model::UserId;

/// Everything `layer_on_call` needs about a single rotation layer — a flattened, resolved view
/// (the service layer joins `RotationLayer` + its ordered `RotationParticipant`s into this).
#[derive(Debug, Clone, Copy)]
pub struct ResolvedLayer<'a> {
    /// Higher priority wins where it has coverage (a non-empty `participants`).
    pub priority: i32,
    pub kind: RotationKind,
    /// The UTC instant at which `participants[0]`'s first shift starts.
    pub anchor_at: DateTime<Utc>,
    /// Ordered by `RotationParticipant::position`.
    pub participants: &'a [UserId],
}

/// Everything needed to answer "who is on call at `now`" — passed by value/ref, no repos.
#[derive(Debug, Clone, Copy)]
pub struct ScheduleSnapshot<'a> {
    /// The schedule's IANA timezone — see `OnCallSchedule::timezone`. Used for `Daily`/`Weekly`
    /// wall-clock handoff math; irrelevant for `Custom` layers (pure UTC duration).
    pub timezone: Tz,
    pub layers: &'a [ResolvedLayer<'a>],
    pub overrides: &'a [OnCallOverride],
}

/// Resolve the effective on-call user at `now`.
///
/// **Resolution order:** (1) an [`OnCallOverride`] with `start_at <= now < end_at` wins outright;
/// on overlapping overrides, the **latest-created** wins the tie — approximated by the greatest
/// [`crate::oncall::model::OnCallOverrideId`], since ids are UUIDv7 and therefore creation-time-ordered.
/// (2) else the highest-`priority` layer whose participant list is non-empty yields
/// [`layer_on_call`]. Ties in `priority` resolve in `snapshot.layers` iteration order
/// (undefined beyond "deterministic for a given input" — callers should avoid creating two
/// layers with equal priority if strict precedence matters).
/// `None` if no override applies and every layer is empty (or there are no layers at all).
pub fn resolve_on_call(snapshot: &ScheduleSnapshot, now: DateTime<Utc>) -> Option<UserId> {
    if let Some(user_id) = resolve_override(snapshot.overrides, now) {
        return Some(user_id);
    }

    let mut layers: Vec<&ResolvedLayer> = snapshot.layers.iter().collect();
    layers.sort_by_key(|l| std::cmp::Reverse(l.priority));

    for layer in layers {
        if let Some(user_id) = layer_on_call(layer, snapshot.timezone, now) {
            return Some(user_id);
        }
    }

    None
}

fn resolve_override(overrides: &[OnCallOverride], now: DateTime<Utc>) -> Option<UserId> {
    overrides
        .iter()
        .filter(|o| o.start_at <= now && now < o.end_at)
        .max_by_key(|o| uuid::Uuid::from(o.id))
        .map(|o| o.user_id)
}

/// The shift index for a single layer at `now`: the count of handoff boundaries between
/// `layer.anchor_at` and `now`, computed in `tz` for `Daily`/`Weekly` (so DST adds/removes no
/// phantom shift), or as `floor((now - anchor_at) / interval_seconds)` for `Custom`.
/// `participants[shift_index mod len]` is on call. Returns `None` if `participants` is empty.
pub fn layer_on_call(layer: &ResolvedLayer, tz: Tz, now: DateTime<Utc>) -> Option<UserId> {
    if layer.participants.is_empty() {
        return None;
    }

    let shift_index = match layer.kind {
        RotationKind::Daily { handoff_time } => {
            let anchor_local = layer.anchor_at.with_timezone(&tz);
            let now_local = now.with_timezone(&tz);
            let base_date = anchor_local.date_naive();
            shift_index_periodic(base_date, 1, handoff_time, tz, anchor_local, now_local)
        }
        RotationKind::Weekly {
            handoff_weekday,
            handoff_time,
        } => {
            let anchor_local = layer.anchor_at.with_timezone(&tz);
            let now_local = now.with_timezone(&tz);
            let base_date = next_or_same_weekday(anchor_local.date_naive(), handoff_weekday);
            shift_index_periodic(base_date, 7, handoff_time, tz, anchor_local, now_local)
        }
        RotationKind::Custom { interval_seconds } => {
            if interval_seconds <= 0 {
                // Degenerate config (should be rejected at the service/validation layer) — a
                // pure function must not panic (div-by-zero) on bad input, so treat as shift 0.
                0
            } else {
                (now - layer.anchor_at)
                    .num_seconds()
                    .div_euclid(interval_seconds)
            }
        }
    };

    let len = layer.participants.len() as i64;
    let idx = shift_index.rem_euclid(len) as usize;
    Some(layer.participants[idx])
}

/// The next handoff boundary for `layer` strictly after `now`, in UTC — used to answer "when
/// does the current shift end" (e.g. for [`crate::oncall::resolve`]'s consumers building an
/// "on-call now" summary). `None` if the layer has no participants (nothing is rotating) or a
/// degenerate `Custom` config (`interval_seconds <= 0`).
pub fn next_boundary(layer: &ResolvedLayer, tz: Tz, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    if layer.participants.is_empty() {
        return None;
    }

    match layer.kind {
        RotationKind::Daily { handoff_time } => {
            let now_local = now.with_timezone(&tz);
            let base_date = layer.anchor_at.with_timezone(&tz).date_naive();
            let k = boundaries_leq(base_date, 1, handoff_time, tz, now_local);
            Some(boundary_datetime(base_date, 1, handoff_time, tz, k + 1).with_timezone(&Utc))
        }
        RotationKind::Weekly {
            handoff_weekday,
            handoff_time,
        } => {
            let now_local = now.with_timezone(&tz);
            let base_date = next_or_same_weekday(
                layer.anchor_at.with_timezone(&tz).date_naive(),
                handoff_weekday,
            );
            let k = boundaries_leq(base_date, 7, handoff_time, tz, now_local);
            Some(boundary_datetime(base_date, 7, handoff_time, tz, k + 1).with_timezone(&Utc))
        }
        RotationKind::Custom { interval_seconds } => {
            if interval_seconds <= 0 {
                None
            } else {
                let shift_index = (now - layer.anchor_at)
                    .num_seconds()
                    .div_euclid(interval_seconds);
                Some(layer.anchor_at + Duration::seconds((shift_index + 1) * interval_seconds))
            }
        }
    }
}

/// One contiguous block of on-call coverage: `user_id` (or `None` if nobody is covering) is
/// constant across `[start_at, end_at)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OnCallShift {
    pub user_id: Option<UserId>,
    pub start_at: DateTime<Utc>,
    pub end_at: DateTime<Utc>,
    /// `true` if `user_id` was resolved from an [`OnCallOverride`] covering this block rather
    /// than the base rotation layer (§10 FLAG B) — lets the frontend render override segments
    /// distinctly (e.g. the `RotationPreviewStrip`).
    pub is_override: bool,
}

/// Resolve on-call coverage over `[from, to)` as a run-length-encoded sequence of shifts —
/// `resolve_on_call` integrated over a range, sharing exactly the same DST/override math (this
/// function is defined purely in terms of [`resolve_on_call`] and boundary enumeration; it never
/// re-derives the resolution rules). Returns an empty `Vec` if `from >= to`.
///
/// **Invariant this function must uphold** (enforced by a property test): for any `t` in
/// `[from, to)`, the shift containing `t` names the same user as `resolve_on_call(snapshot, t)`.
/// The two must never diverge.
///
/// The **caller is responsible for bounding `to - from`** (the service layer rejects spans over
/// 90 days) and `interval_seconds` (the service layer's `Custom`-rotation floor) — legitimate,
/// validated calls never come close to enumerating [`MAX_CUSTOM_BOUNDARIES`] boundaries. That
/// constant exists purely as defense-in-depth *within this function* (C1 security review): a
/// caller that somehow bypasses both those checks (e.g. a schedule persisted with a tiny
/// `interval_seconds` before the service-layer floor existed) still cannot make a single
/// `resolve_shifts` call enumerate more than a bounded number of boundaries — collection simply
/// stops early past the cap rather than walking an unbounded number of them (a `Custom` layer
/// with `interval_seconds: 1` over 90 days would otherwise enumerate ~7.8M).
pub fn resolve_shifts(
    snapshot: &ScheduleSnapshot,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> Vec<OnCallShift> {
    if from >= to {
        return Vec::new();
    }

    // Layer priority/emptiness is static per snapshot (participants don't change over time within
    // a single resolution call), so exactly one layer can ever be the "base" winner for the whole
    // range — only overrides can additionally override it, transiently.
    let mut layers: Vec<&ResolvedLayer> = snapshot.layers.iter().collect();
    layers.sort_by_key(|l| std::cmp::Reverse(l.priority));
    let base_layer = layers.into_iter().find(|l| !l.participants.is_empty());

    let mut boundaries: Vec<DateTime<Utc>> = Vec::new();
    if let Some(layer) = base_layer {
        collect_layer_boundaries(layer, snapshot.timezone, from, to, &mut boundaries);
    }
    for o in snapshot.overrides {
        if o.start_at > from && o.start_at < to {
            boundaries.push(o.start_at);
        }
        if o.end_at > from && o.end_at < to {
            boundaries.push(o.end_at);
        }
    }
    boundaries.sort();
    boundaries.dedup();

    let mut points = Vec::with_capacity(boundaries.len() + 2);
    points.push(from);
    points.append(&mut boundaries);
    points.push(to);

    let mut shifts: Vec<OnCallShift> = Vec::new();
    for pair in points.windows(2) {
        let (seg_start, seg_end) = (pair[0], pair[1]);
        if seg_start >= seg_end {
            continue; // guards against a boundary landing exactly on `from`/`to`
        }
        let user_id = resolve_on_call(snapshot, seg_start);
        let is_override = resolve_override(snapshot.overrides, seg_start).is_some();
        match shifts.last_mut() {
            Some(last)
                if last.user_id == user_id
                    && last.is_override == is_override
                    && last.end_at == seg_start =>
            {
                last.end_at = seg_end;
            }
            _ => shifts.push(OnCallShift {
                user_id,
                start_at: seg_start,
                end_at: seg_end,
                is_override,
            }),
        }
    }
    shifts
}

/// Hard backstop (C1 security review) bounding how many boundaries a single `resolve_shifts`
/// call will ever enumerate for a `Custom` layer, regardless of how small `interval_seconds` is.
/// Chosen with headroom above the legitimate worst case under the service layer's own bounds
/// (90-day max span / 300s minimum `interval_seconds` = 25,920 boundaries — see
/// `rim_infra_services::oncall::MIN_CUSTOM_INTERVAL_SECONDS`), so this never rejects a
/// validated, legitimate query; it only stops a caller that has bypassed those bounds from
/// making this function do unbounded work.
const MAX_CUSTOM_BOUNDARIES: usize = 50_000;

/// Appends every handoff boundary of `layer` strictly inside `(from, to)` to `out`, in `tz`. A
/// `Custom` layer with `interval_seconds <= 0` (degenerate) contributes no boundaries, matching
/// [`layer_on_call`]'s treatment of the same case. Stops early, without enumerating further
/// boundaries, once [`MAX_CUSTOM_BOUNDARIES`] is reached (see that constant's doc comment).
fn collect_layer_boundaries(
    layer: &ResolvedLayer,
    tz: Tz,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    out: &mut Vec<DateTime<Utc>>,
) {
    match layer.kind {
        RotationKind::Daily { handoff_time } => {
            let base_date = layer.anchor_at.with_timezone(&tz).date_naive();
            collect_periodic_boundaries(base_date, 1, handoff_time, tz, from, to, out);
        }
        RotationKind::Weekly {
            handoff_weekday,
            handoff_time,
        } => {
            let base_date = next_or_same_weekday(
                layer.anchor_at.with_timezone(&tz).date_naive(),
                handoff_weekday,
            );
            collect_periodic_boundaries(base_date, 7, handoff_time, tz, from, to, out);
        }
        RotationKind::Custom { interval_seconds } => {
            if interval_seconds <= 0 {
                return;
            }
            let k_from = (from - layer.anchor_at)
                .num_seconds()
                .div_euclid(interval_seconds);
            let k_to = (to - layer.anchor_at)
                .num_seconds()
                .div_euclid(interval_seconds);
            // +1 on each end to be generous with the estimate; the `t > from && t < to` filter
            // below is what actually enforces correctness, not this range.
            for k in (k_from - 1)..=(k_to + 1) {
                if out.len() >= MAX_CUSTOM_BOUNDARIES {
                    break;
                }
                let t = layer.anchor_at + Duration::seconds(k * interval_seconds);
                if t > from && t < to {
                    out.push(t);
                }
            }
        }
    }
}

/// Appends every periodic boundary (`base_date + k * period_days` at `handoff_time`) strictly
/// inside `(from, to)` to `out`, in `tz`.
fn collect_periodic_boundaries(
    base_date: NaiveDate,
    period_days: i64,
    handoff_time: NaiveTime,
    tz: Tz,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    out: &mut Vec<DateTime<Utc>>,
) {
    let from_local = from.with_timezone(&tz);
    let to_local = to.with_timezone(&tz);
    let k_from = boundaries_leq(base_date, period_days, handoff_time, tz, from_local);
    let k_to = boundaries_leq(base_date, period_days, handoff_time, tz, to_local);
    for k in (k_from + 1)..=(k_to + 1) {
        let t = boundary_datetime(base_date, period_days, handoff_time, tz, k).with_timezone(&Utc);
        if t > from && t < to {
            out.push(t);
        }
    }
}

/// The first date `>= from` whose weekday is `target` (`from` itself if it already matches).
fn next_or_same_weekday(from: NaiveDate, target: Weekday) -> NaiveDate {
    let delta = (target.num_days_from_monday() as i64
        - from.weekday().num_days_from_monday() as i64)
        .rem_euclid(7);
    from + Duration::days(delta)
}

/// Resolves a naive local wall-clock instant to a concrete UTC-backed `DateTime<Tz>`, applying
/// the two documented DST edge-case choices:
/// - **Spring-forward gap** (the naive time doesn't exist, e.g. 02:30 on a 02:00→03:00 jump):
///   resolve forward to the next valid instant.
/// - **Fall-back overlap** (the naive time occurs twice): take the first (earlier UTC) occurrence.
fn resolve_local(tz: Tz, naive: NaiveDateTime) -> DateTime<Tz> {
    use chrono::TimeZone;
    match tz.from_local_datetime(&naive) {
        chrono::LocalResult::Single(dt) => dt,
        chrono::LocalResult::Ambiguous(earlier, _later) => earlier,
        chrono::LocalResult::None => {
            // Spring-forward gap: probe forward in small steps until we land on (or past) a
            // valid local instant. DST gaps are at most a couple of hours in real tzdata, so
            // this converges quickly; the loop has no fixed bound because correctness (never
            // silently returning a wrong instant) matters more than a defensive cap here — an
            // IANA tz database with a gap that never resolves is not a real-world case.
            let mut probe = naive;
            loop {
                probe += Duration::minutes(1);
                match tz.from_local_datetime(&probe) {
                    chrono::LocalResult::Single(dt) => return dt,
                    chrono::LocalResult::Ambiguous(earlier, _later) => return earlier,
                    chrono::LocalResult::None => continue,
                }
            }
        }
    }
}

/// Counts how many periodic handoff boundaries (`base_date + k * period_days` at `handoff_time`,
/// for integer `k`) fall in `(anchor_local, now_local]`, by computing "boundaries `<=` an instant"
/// for both ends and subtracting — which correctly handles the half-open interval regardless of
/// whether `anchor_local` itself coincides with a boundary.
fn shift_index_periodic(
    base_date: NaiveDate,
    period_days: i64,
    handoff_time: NaiveTime,
    tz: Tz,
    anchor_local: DateTime<Tz>,
    now_local: DateTime<Tz>,
) -> i64 {
    boundaries_leq(base_date, period_days, handoff_time, tz, now_local)
        - boundaries_leq(base_date, period_days, handoff_time, tz, anchor_local)
}

/// The boundary instant at index `k`: `base_date + k * period_days`, at `handoff_time`,
/// DST-resolved via [`resolve_local`].
fn boundary_datetime(
    base_date: NaiveDate,
    period_days: i64,
    handoff_time: NaiveTime,
    tz: Tz,
    k: i64,
) -> DateTime<Tz> {
    let date = base_date + Duration::days(k * period_days);
    resolve_local(tz, NaiveDateTime::new(date, handoff_time))
}

/// The largest `k` such that the boundary at `base_date + k * period_days` (at `handoff_time`,
/// DST-resolved via [`resolve_local`]) is `<= instant`. `base_date` need not itself be a boundary
/// date already aligned to the desired phase — any date works as long as every `base_date + k *
/// period_days` is one of the intended handoff dates (e.g., for `Weekly`, `base_date` must be a
/// date whose weekday matches the target handoff weekday).
fn boundaries_leq(
    base_date: NaiveDate,
    period_days: i64,
    handoff_time: NaiveTime,
    tz: Tz,
    instant: DateTime<Tz>,
) -> i64 {
    let days_diff = (instant.date_naive() - base_date).num_days();
    let mut k = days_diff.div_euclid(period_days);

    let candidate_at =
        |k: i64| -> DateTime<Tz> { boundary_datetime(base_date, period_days, handoff_time, tz, k) };

    // `k` from the date-only estimate above is within one period of the true answer; walk it to
    // the exact boundary. Bounded defensively (a correct estimate never needs more than a
    // couple of steps) so a pure function can never loop forever on unexpected input.
    for _ in 0..8 {
        if candidate_at(k) <= instant {
            if candidate_at(k + 1) <= instant {
                k += 1;
            } else {
                return k;
            }
        } else {
            k -= 1;
        }
    }
    k
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oncall::model::{OnCallOverrideId, OnCallScheduleId};
    use chrono::{Offset, TimeZone};
    use proptest::prelude::*;

    fn uid() -> UserId {
        UserId::new()
    }

    fn utc(y: i32, m: u32, d: u32, h: u32, mi: u32, s: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, mi, s).unwrap()
    }

    // ── Empty schedule / override precedence (unit) ─────────────────────────

    #[test]
    fn empty_schedule_resolves_to_none() {
        let snapshot = ScheduleSnapshot {
            timezone: chrono_tz::UTC,
            layers: &[],
            overrides: &[],
        };
        assert_eq!(resolve_on_call(&snapshot, utc(2026, 1, 1, 0, 0, 0)), None);
    }

    #[test]
    fn layer_with_no_participants_is_skipped_in_favor_of_lower_priority_layer_with_some() {
        let u = uid();
        let empty_layer = ResolvedLayer {
            priority: 10,
            kind: RotationKind::Custom {
                interval_seconds: 3600,
            },
            anchor_at: utc(2026, 1, 1, 0, 0, 0),
            participants: &[],
        };
        let covering_layer = ResolvedLayer {
            priority: 1,
            kind: RotationKind::Custom {
                interval_seconds: 3600,
            },
            anchor_at: utc(2026, 1, 1, 0, 0, 0),
            participants: &[u],
        };
        let snapshot = ScheduleSnapshot {
            timezone: chrono_tz::UTC,
            layers: &[empty_layer, covering_layer],
            overrides: &[],
        };
        assert_eq!(
            resolve_on_call(&snapshot, utc(2026, 1, 1, 0, 0, 0)),
            Some(u)
        );
    }

    #[test]
    fn higher_priority_layer_wins_when_both_have_participants() {
        let high = uid();
        let low = uid();
        let high_layer = ResolvedLayer {
            priority: 10,
            kind: RotationKind::Custom {
                interval_seconds: 3600,
            },
            anchor_at: utc(2026, 1, 1, 0, 0, 0),
            participants: std::slice::from_ref(&high),
        };
        let low_layer = ResolvedLayer {
            priority: 1,
            kind: RotationKind::Custom {
                interval_seconds: 3600,
            },
            anchor_at: utc(2026, 1, 1, 0, 0, 0),
            participants: std::slice::from_ref(&low),
        };
        let snapshot = ScheduleSnapshot {
            timezone: chrono_tz::UTC,
            layers: &[low_layer, high_layer],
            overrides: &[],
        };
        assert_eq!(
            resolve_on_call(&snapshot, utc(2026, 1, 1, 0, 0, 0)),
            Some(high)
        );
    }

    #[test]
    fn override_covering_now_beats_any_layer() {
        let layer_user = uid();
        let override_user = uid();
        let layer = ResolvedLayer {
            priority: 100,
            kind: RotationKind::Custom {
                interval_seconds: 3600,
            },
            anchor_at: utc(2026, 1, 1, 0, 0, 0),
            participants: std::slice::from_ref(&layer_user),
        };
        let over = OnCallOverride {
            id: OnCallOverrideId::new(),
            schedule_id: crate::oncall::model::OnCallScheduleId::new(),
            user_id: override_user,
            start_at: utc(2026, 1, 1, 0, 0, 0),
            end_at: utc(2026, 1, 2, 0, 0, 0),
            created_by: uid(),
        };
        let snapshot = ScheduleSnapshot {
            timezone: chrono_tz::UTC,
            layers: &[layer],
            overrides: std::slice::from_ref(&over),
        };
        assert_eq!(
            resolve_on_call(&snapshot, utc(2026, 1, 1, 12, 0, 0)),
            Some(override_user)
        );
        // Just outside [start, end) — the layer applies again.
        assert_eq!(
            resolve_on_call(&snapshot, utc(2026, 1, 2, 0, 0, 0)),
            Some(layer_user)
        );
    }

    #[test]
    fn later_created_override_wins_on_overlap() {
        // UUIDv7 ids are time-ordered, so constructing the "later" override second (`OnCallOverrideId::new()`
        // called after) simulates "created later".
        let earlier_user = uid();
        let earlier = OnCallOverride {
            id: OnCallOverrideId::new(),
            schedule_id: crate::oncall::model::OnCallScheduleId::new(),
            user_id: earlier_user,
            start_at: utc(2026, 1, 1, 0, 0, 0),
            end_at: utc(2026, 1, 3, 0, 0, 0),
            created_by: uid(),
        };
        let later_user = uid();
        let later = OnCallOverride {
            id: OnCallOverrideId::new(),
            schedule_id: crate::oncall::model::OnCallScheduleId::new(),
            user_id: later_user,
            start_at: utc(2026, 1, 1, 12, 0, 0),
            end_at: utc(2026, 1, 2, 0, 0, 0),
            created_by: uid(),
        };
        let snapshot = ScheduleSnapshot {
            timezone: chrono_tz::UTC,
            layers: &[],
            overrides: &[earlier, later],
        };
        assert_eq!(
            resolve_on_call(&snapshot, utc(2026, 1, 1, 18, 0, 0)),
            Some(later_user)
        );
    }

    // ── Custom (pure UTC duration) round-robin (unit) ───────────────────────

    #[test]
    fn custom_round_robin_advances_by_interval() {
        let u0 = uid();
        let u1 = uid();
        let u2 = uid();
        let anchor = utc(2026, 1, 1, 0, 0, 0);
        let layer = ResolvedLayer {
            priority: 0,
            kind: RotationKind::Custom {
                interval_seconds: 3600,
            },
            anchor_at: anchor,
            participants: &[u0, u1, u2],
        };
        let snapshot = ScheduleSnapshot {
            timezone: chrono_tz::UTC,
            layers: &[layer],
            overrides: &[],
        };
        assert_eq!(resolve_on_call(&snapshot, anchor), Some(u0));
        assert_eq!(
            resolve_on_call(&snapshot, anchor + Duration::minutes(30)),
            Some(u0)
        );
        assert_eq!(
            resolve_on_call(&snapshot, anchor + Duration::hours(1)),
            Some(u1)
        );
        assert_eq!(
            resolve_on_call(&snapshot, anchor + Duration::hours(2)),
            Some(u2)
        );
        assert_eq!(
            resolve_on_call(&snapshot, anchor + Duration::hours(3)),
            Some(u0)
        );
        // Before the anchor: still well-defined (negative shift, wraps via rem_euclid).
        assert_eq!(
            resolve_on_call(&snapshot, anchor - Duration::hours(1)),
            Some(u2)
        );
    }

    // ── next_boundary (unit) ─────────────────────────────────────────────────

    #[test]
    fn next_boundary_custom_is_anchor_plus_interval() {
        let u0 = uid();
        let anchor = utc(2026, 1, 1, 0, 0, 0);
        let layer = ResolvedLayer {
            priority: 0,
            kind: RotationKind::Custom {
                interval_seconds: 3600,
            },
            anchor_at: anchor,
            participants: std::slice::from_ref(&u0),
        };
        assert_eq!(
            next_boundary(&layer, chrono_tz::UTC, anchor),
            Some(anchor + Duration::hours(1))
        );
        assert_eq!(
            next_boundary(&layer, chrono_tz::UTC, anchor + Duration::minutes(30)),
            Some(anchor + Duration::hours(1))
        );
    }

    #[test]
    fn next_boundary_daily_is_next_days_handoff() {
        let u0 = uid();
        let u1 = uid();
        let tz: Tz = "America/Chicago".parse().unwrap();
        let anchor = tz
            .with_ymd_and_hms(2026, 1, 5, 9, 0, 0)
            .unwrap()
            .with_timezone(&Utc);
        let layer = ResolvedLayer {
            priority: 0,
            kind: RotationKind::Daily {
                handoff_time: NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
            },
            anchor_at: anchor,
            participants: &[u0, u1],
        };
        assert_eq!(
            next_boundary(&layer, tz, anchor + Duration::hours(1)),
            Some(anchor + Duration::hours(24))
        );
    }

    #[test]
    fn next_boundary_is_none_for_empty_participants_or_degenerate_custom() {
        let anchor = utc(2026, 1, 1, 0, 0, 0);
        let empty_layer = ResolvedLayer {
            priority: 0,
            kind: RotationKind::Custom {
                interval_seconds: 3600,
            },
            anchor_at: anchor,
            participants: &[],
        };
        assert_eq!(next_boundary(&empty_layer, chrono_tz::UTC, anchor), None);

        let u0 = uid();
        let degenerate_layer = ResolvedLayer {
            priority: 0,
            kind: RotationKind::Custom {
                interval_seconds: 0,
            },
            anchor_at: anchor,
            participants: std::slice::from_ref(&u0),
        };
        assert_eq!(
            next_boundary(&degenerate_layer, chrono_tz::UTC, anchor),
            None
        );
    }

    // ── Daily handoff (unit) ─────────────────────────────────────────────────

    #[test]
    fn daily_handoff_switches_at_local_wall_clock_time() {
        let u0 = uid();
        let u1 = uid();
        let tz: Tz = "America/Chicago".parse().unwrap();
        // Anchor at 09:00 CST on 2026-01-05 (a date with no DST transition nearby).
        let anchor = tz
            .with_ymd_and_hms(2026, 1, 5, 9, 0, 0)
            .unwrap()
            .with_timezone(&Utc);
        let layer = ResolvedLayer {
            priority: 0,
            kind: RotationKind::Daily {
                handoff_time: NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
            },
            anchor_at: anchor,
            participants: &[u0, u1],
        };
        let snapshot = ScheduleSnapshot {
            timezone: tz,
            layers: &[layer],
            overrides: &[],
        };
        assert_eq!(resolve_on_call(&snapshot, anchor), Some(u0));
        assert_eq!(
            resolve_on_call(&snapshot, anchor + Duration::hours(23)),
            Some(u0)
        );
        assert_eq!(
            resolve_on_call(&snapshot, anchor + Duration::hours(24)),
            Some(u1)
        );
    }

    // ── DST edge instants (unit, explicit per §7) ────────────────────────────
    // America/Chicago 2026: spring-forward 2026-03-08 02:00 -> 03:00 CDT;
    // fall-back 2026-11-01 02:00 CDT -> 01:00 CST.

    #[test]
    fn spring_forward_gap_resolves_forward_to_next_valid_instant() {
        let tz: Tz = "America/Chicago".parse().unwrap();
        let naive = NaiveDateTime::new(
            NaiveDate::from_ymd_opt(2026, 3, 8).unwrap(),
            NaiveTime::from_hms_opt(2, 30, 0).unwrap(), // in the gap: doesn't exist
        );
        let resolved = resolve_local(tz, naive);
        // The gap jumps 02:00 -> 03:00, so 02:30 resolves to 03:00 CDT (documented choice).
        assert_eq!(resolved.format("%H:%M").to_string(), "03:00");
        assert_eq!(resolved.date_naive(), naive.date());
    }

    #[test]
    fn fall_back_overlap_resolves_to_first_occurrence() {
        let tz: Tz = "America/Chicago".parse().unwrap();
        let naive = NaiveDateTime::new(
            NaiveDate::from_ymd_opt(2026, 11, 1).unwrap(),
            NaiveTime::from_hms_opt(1, 30, 0).unwrap(), // occurs twice: CDT then CST
        );
        let resolved = resolve_local(tz, naive);
        // The earlier occurrence is the CDT (UTC-5) one, i.e. -5 * 3600 seconds from UTC.
        assert_eq!(resolved.offset().fix().local_minus_utc(), -5 * 3600);
    }

    #[test]
    fn weekly_handoff_across_spring_forward_hands_off_exactly_once() {
        let u0 = uid();
        let u1 = uid();
        let tz: Tz = "America/Chicago".parse().unwrap();
        // Anchor on a Sunday well before the spring-forward weekend, handoff Sundays at 02:30
        // (inside the gap on 2026-03-08, forcing the gap-resolution path).
        let anchor = tz
            .with_ymd_and_hms(2026, 3, 1, 2, 30, 0)
            .unwrap()
            .with_timezone(&Utc);
        let layer = ResolvedLayer {
            priority: 0,
            kind: RotationKind::Weekly {
                handoff_weekday: Weekday::Sun,
                handoff_time: NaiveTime::from_hms_opt(2, 30, 0).unwrap(),
            },
            anchor_at: anchor,
            participants: &[u0, u1],
        };
        let snapshot = ScheduleSnapshot {
            timezone: tz,
            layers: &[layer],
            overrides: &[],
        };
        // The handoff instant itself: 02:30 doesn't exist on 2026-03-08 (clocks jump 02:00 ->
        // 03:00), so it resolves forward to 03:00 CDT. Derive the probes from that resolved
        // instant via plain UTC arithmetic — constructing e.g. "02:59 local" directly would
        // itself fall inside the gap and panic.
        let handoff_instant = resolve_local(
            tz,
            NaiveDateTime::new(
                NaiveDate::from_ymd_opt(2026, 3, 8).unwrap(),
                NaiveTime::from_hms_opt(2, 30, 0).unwrap(),
            ),
        )
        .with_timezone(&Utc);
        let just_before = handoff_instant - Duration::minutes(1);
        assert_eq!(resolve_on_call(&snapshot, just_before), Some(u0));
        // Just after: exactly one handoff has occurred, not two.
        let just_after = handoff_instant + Duration::minutes(1);
        assert_eq!(resolve_on_call(&snapshot, just_after), Some(u1));
        // A full week later: back to u0 (exactly one further handoff, not skipped or doubled).
        let next_week = just_after + Duration::days(7);
        assert_eq!(resolve_on_call(&snapshot, next_week), Some(u0));
    }

    #[test]
    fn weekly_handoff_across_fall_back_hands_off_exactly_once() {
        let u0 = uid();
        let u1 = uid();
        let tz: Tz = "America/Chicago".parse().unwrap();
        let anchor = tz
            .with_ymd_and_hms(2026, 10, 25, 9, 0, 0)
            .unwrap()
            .with_timezone(&Utc);
        let layer = ResolvedLayer {
            priority: 0,
            kind: RotationKind::Weekly {
                handoff_weekday: Weekday::Sun,
                handoff_time: NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
            },
            anchor_at: anchor,
            participants: &[u0, u1],
        };
        let snapshot = ScheduleSnapshot {
            timezone: tz,
            layers: &[layer],
            overrides: &[],
        };
        let before = tz
            .with_ymd_and_hms(2026, 11, 1, 8, 59, 0)
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(resolve_on_call(&snapshot, before), Some(u0));
        let after = tz
            .with_ymd_and_hms(2026, 11, 1, 9, 1, 0)
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(resolve_on_call(&snapshot, after), Some(u1));
        let next_week = after + Duration::days(7);
        assert_eq!(resolve_on_call(&snapshot, next_week), Some(u0));
    }

    // ── resolve_shifts (unit) ─────────────────────────────────────────────────

    #[test]
    fn resolve_shifts_empty_range_returns_empty() {
        let snapshot = ScheduleSnapshot {
            timezone: chrono_tz::UTC,
            layers: &[],
            overrides: &[],
        };
        let now = utc(2026, 1, 1, 0, 0, 0);
        assert!(resolve_shifts(&snapshot, now, now).is_empty());
        assert!(resolve_shifts(&snapshot, now, now - Duration::seconds(1)).is_empty());
    }

    #[test]
    fn resolve_shifts_custom_layer_produces_one_shift_per_participant() {
        let u0 = uid();
        let u1 = uid();
        let u2 = uid();
        let anchor = utc(2026, 1, 1, 0, 0, 0);
        let layer = ResolvedLayer {
            priority: 0,
            kind: RotationKind::Custom {
                interval_seconds: 3600,
            },
            anchor_at: anchor,
            participants: &[u0, u1, u2],
        };
        let snapshot = ScheduleSnapshot {
            timezone: chrono_tz::UTC,
            layers: &[layer],
            overrides: &[],
        };

        let shifts = resolve_shifts(&snapshot, anchor, anchor + Duration::hours(3));
        assert_eq!(shifts.len(), 3);
        assert_eq!(shifts[0].user_id, Some(u0));
        assert_eq!(shifts[0].start_at, anchor);
        assert_eq!(shifts[0].end_at, anchor + Duration::hours(1));
        assert_eq!(shifts[1].user_id, Some(u1));
        assert_eq!(shifts[1].start_at, anchor + Duration::hours(1));
        assert_eq!(shifts[1].end_at, anchor + Duration::hours(2));
        assert_eq!(shifts[2].user_id, Some(u2));
        assert_eq!(shifts[2].start_at, anchor + Duration::hours(2));
        assert_eq!(shifts[2].end_at, anchor + Duration::hours(3));
    }

    #[test]
    fn resolve_shifts_merges_segments_with_no_boundary_in_range() {
        let u0 = uid();
        let anchor = utc(2026, 1, 1, 0, 0, 0);
        // A single-participant layer never hands off — the whole queried range should collapse
        // into exactly one shift, even though the range spans many "would-be" boundaries.
        let layer = ResolvedLayer {
            priority: 0,
            kind: RotationKind::Custom {
                interval_seconds: 3600,
            },
            anchor_at: anchor,
            participants: &[u0],
        };
        let snapshot = ScheduleSnapshot {
            timezone: chrono_tz::UTC,
            layers: &[layer],
            overrides: &[],
        };

        let shifts = resolve_shifts(&snapshot, anchor, anchor + Duration::hours(10));
        assert_eq!(shifts.len(), 1);
        assert_eq!(shifts[0].user_id, Some(u0));
        assert_eq!(shifts[0].start_at, anchor);
        assert_eq!(shifts[0].end_at, anchor + Duration::hours(10));
    }

    #[test]
    fn resolve_shifts_splits_at_override_boundaries() {
        let layer_user = uid();
        let override_user = uid();
        let anchor = utc(2026, 1, 1, 0, 0, 0);
        let layer = ResolvedLayer {
            priority: 0,
            kind: RotationKind::Custom {
                interval_seconds: 36_000, // long enough that no layer boundary falls in range
            },
            anchor_at: anchor,
            participants: std::slice::from_ref(&layer_user),
        };
        let over = OnCallOverride {
            id: crate::oncall::model::OnCallOverrideId::new(),
            schedule_id: crate::oncall::model::OnCallScheduleId::new(),
            user_id: override_user,
            start_at: anchor + Duration::hours(1),
            end_at: anchor + Duration::hours(2),
            created_by: uid(),
        };
        let snapshot = ScheduleSnapshot {
            timezone: chrono_tz::UTC,
            layers: &[layer],
            overrides: std::slice::from_ref(&over),
        };

        let shifts = resolve_shifts(&snapshot, anchor, anchor + Duration::hours(3));
        assert_eq!(shifts.len(), 3);
        assert_eq!(shifts[0].user_id, Some(layer_user));
        assert_eq!(shifts[0].end_at, anchor + Duration::hours(1));
        assert!(
            !shifts[0].is_override,
            "rotation segment must not be flagged as an override"
        );
        assert_eq!(shifts[1].user_id, Some(override_user));
        assert_eq!(shifts[1].start_at, anchor + Duration::hours(1));
        assert_eq!(shifts[1].end_at, anchor + Duration::hours(2));
        assert!(
            shifts[1].is_override,
            "override segment must be flagged is_override"
        );
        assert_eq!(shifts[2].user_id, Some(layer_user));
        assert_eq!(shifts[2].start_at, anchor + Duration::hours(2));
        assert_eq!(shifts[2].end_at, anchor + Duration::hours(3));
        assert!(
            !shifts[2].is_override,
            "rotation segment must not be flagged as an override"
        );
    }

    /// C1 security review — hard backstop regression test: a `Custom` layer with
    /// `interval_seconds: 1` over a 90-day span would enumerate ~7.8M boundaries without the
    /// `MAX_CUSTOM_BOUNDARIES` cap. This deliberately bypasses the service-layer floor/span
    /// checks entirely (calling `resolve_shifts` directly, as a caller that skipped validation
    /// would) to prove `resolve_shifts` itself never walks more than the capped number of
    /// boundaries — the test completing quickly, rather than enumerating millions of points, IS
    /// the assertion.
    #[test]
    fn resolve_shifts_caps_boundary_count_for_pathological_custom_interval() {
        let u0 = uid();
        let u1 = uid();
        let anchor = utc(2026, 1, 1, 0, 0, 0);
        let layer = ResolvedLayer {
            priority: 0,
            kind: RotationKind::Custom {
                interval_seconds: 1,
            },
            anchor_at: anchor,
            participants: &[u0, u1],
        };
        let snapshot = ScheduleSnapshot {
            timezone: chrono_tz::UTC,
            layers: &[layer],
            overrides: &[],
        };

        let shifts = resolve_shifts(&snapshot, anchor, anchor + Duration::days(90));
        // Boundaries are capped at `MAX_CUSTOM_BOUNDARIES`, so there can be at most that many
        // shifts (a boundary starts a new shift only when the user actually changes, so the
        // real count is normally lower, but it must never exceed boundaries + 1).
        assert!(shifts.len() <= MAX_CUSTOM_BOUNDARIES + 1);
    }

    // ── Property tests (mandated, §7) ────────────────────────────────────────

    fn user_ids(n: usize) -> Vec<UserId> {
        (0..n).map(|_| UserId::new()).collect()
    }

    proptest! {
        /// *Monotonic within a shift:* for a `Custom` layer, `resolve_on_call` is constant
        /// between consecutive handoff boundaries and changes only at them.
        #[test]
        fn prop_monotonic_within_shift(
            n_participants in 1usize..6,
            interval_seconds in 60i64..100_000,
            anchor_offset_secs in -1_000_000i64..1_000_000,
            probe_offset_secs in 0i64..100_000,
        ) {
            let participants = user_ids(n_participants);
            let anchor = utc(2026, 1, 1, 0, 0, 0) + Duration::seconds(anchor_offset_secs);
            let layer = ResolvedLayer {
                priority: 0,
                kind: RotationKind::Custom { interval_seconds },
                anchor_at: anchor,
                participants: &participants,
            };
            let snapshot = ScheduleSnapshot { timezone: chrono_tz::UTC, layers: &[layer], overrides: &[] };

            let now = anchor + Duration::seconds(probe_offset_secs);
            let shift_start = anchor
                + Duration::seconds((now - anchor).num_seconds().div_euclid(interval_seconds) * interval_seconds);
            let shift_end = shift_start + Duration::seconds(interval_seconds);

            let at_now = resolve_on_call(&snapshot, now);
            let at_start = resolve_on_call(&snapshot, shift_start);
            let at_end_minus_1s = resolve_on_call(&snapshot, shift_end - Duration::seconds(1));

            prop_assert_eq!(at_now, at_start);
            prop_assert_eq!(at_start, at_end_minus_1s);
        }

        /// *Round-robin coverage:* over N·interval, each of N participants is on call for exactly
        /// one shift per cycle, in `position` order.
        #[test]
        fn prop_round_robin_coverage(
            n_participants in 1usize..8,
            interval_seconds in 60i64..100_000,
        ) {
            let participants = user_ids(n_participants);
            let anchor = utc(2026, 1, 1, 0, 0, 0);
            let layer = ResolvedLayer {
                priority: 0,
                kind: RotationKind::Custom { interval_seconds },
                anchor_at: anchor,
                participants: &participants,
            };
            let snapshot = ScheduleSnapshot { timezone: chrono_tz::UTC, layers: &[layer], overrides: &[] };

            for (i, expected) in participants.iter().enumerate() {
                let probe = anchor + Duration::seconds(interval_seconds * i as i64 + interval_seconds / 2);
                prop_assert_eq!(resolve_on_call(&snapshot, probe), Some(*expected));
            }
            // One full cycle later, coverage repeats identically.
            let cycle = Duration::seconds(interval_seconds * n_participants as i64);
            for (i, expected) in participants.iter().enumerate() {
                let probe = anchor + cycle + Duration::seconds(interval_seconds * i as i64 + interval_seconds / 2);
                prop_assert_eq!(resolve_on_call(&snapshot, probe), Some(*expected));
            }
        }

        /// *Override precedence:* an override covering `now` always wins over any layer,
        /// regardless of layer priority or rotation state.
        #[test]
        fn prop_override_always_wins_when_covering_now(
            n_participants in 1usize..6,
            interval_seconds in 60i64..100_000,
            probe_offset_secs in 0i64..1_000_000,
        ) {
            let participants = user_ids(n_participants);
            let anchor = utc(2026, 1, 1, 0, 0, 0);
            let layer = ResolvedLayer {
                priority: 1000,
                kind: RotationKind::Custom { interval_seconds },
                anchor_at: anchor,
                participants: &participants,
            };
            let now = anchor + Duration::seconds(probe_offset_secs);
            let override_user = UserId::new();
            let over = OnCallOverride {
                id: OnCallOverrideId::new(),
                schedule_id: crate::oncall::model::OnCallScheduleId::new(),
                user_id: override_user,
                start_at: now - Duration::minutes(1),
                end_at: now + Duration::minutes(1),
                created_by: UserId::new(),
            };
            let snapshot = ScheduleSnapshot {
                timezone: chrono_tz::UTC,
                layers: &[layer],
                overrides: std::slice::from_ref(&over),
            };
            prop_assert_eq!(resolve_on_call(&snapshot, now), Some(override_user));
        }

        /// Empty schedule (no layers, no overrides) always resolves to `None`.
        #[test]
        fn prop_empty_schedule_is_always_none(probe_offset_secs in -1_000_000i64..1_000_000) {
            let snapshot = ScheduleSnapshot { timezone: chrono_tz::UTC, layers: &[], overrides: &[] };
            let now = utc(2026, 1, 1, 0, 0, 0) + Duration::seconds(probe_offset_secs);
            prop_assert_eq!(resolve_on_call(&snapshot, now), None);
        }

        /// *`resolve_shifts` must never diverge from `resolve_on_call`:* for any `t` in
        /// `[from, to)`, the shift containing `t` names the same user as calling
        /// `resolve_on_call` directly at `t`. This is the invariant tying the two functions
        /// together (mandated per the C1a scope addition) — `resolve_shifts` must be provably a
        /// thin, range-integrated wrapper around `resolve_on_call`, never a re-derivation.
        #[test]
        fn prop_resolve_shifts_matches_resolve_on_call(
            n_participants in 1usize..5,
            interval_seconds in 60i64..50_000,
            range_hours in 1i64..48,
            has_override in proptest::bool::ANY,
            override_start_offset_hours in 0i64..47,
            override_len_hours in 1i64..5,
            t_fraction_pct in 0i64..100,
        ) {
            let participants = user_ids(n_participants);
            let anchor = utc(2026, 1, 1, 0, 0, 0);
            let layer = ResolvedLayer {
                priority: 0,
                kind: RotationKind::Custom { interval_seconds },
                anchor_at: anchor,
                participants: &participants,
            };
            let from = anchor;
            let to = anchor + Duration::hours(range_hours);

            let over = OnCallOverride {
                id: OnCallOverrideId::new(),
                schedule_id: OnCallScheduleId::new(),
                user_id: UserId::new(),
                start_at: from + Duration::hours(override_start_offset_hours),
                end_at: from + Duration::hours(override_start_offset_hours) + Duration::hours(override_len_hours),
                created_by: UserId::new(),
            };
            let overrides: Vec<OnCallOverride> = if has_override { vec![over] } else { vec![] };
            let snapshot = ScheduleSnapshot { timezone: chrono_tz::UTC, layers: &[layer], overrides: &overrides };

            let shifts = resolve_shifts(&snapshot, from, to);

            let span_secs = (to - from).num_seconds();
            let t = from + Duration::seconds(span_secs * t_fraction_pct / 100);
            prop_assert!(t >= from && t < to);

            let expected = resolve_on_call(&snapshot, t);
            let containing = shifts.iter().find(|s| s.start_at <= t && t < s.end_at);
            prop_assert_eq!(containing.map(|s| s.user_id), Some(expected));
        }
    }
}
