//! Premiere-style timecode, in both directions, drop-frame included.
//!
//! Premiere shows source timecode as `HH:MM:SS:FF` and, on 29.97 and 59.94 material, as
//! `HH:MM:SS;FF` — the semicolon means *drop-frame*. The distinction is not cosmetic. On a
//! 29.97 clip one hour of wall-clock is 107 892 frames, but non-drop timecode counts
//! 108 000 labels, so the two readings of `01:00:00:00` are 108 frames apart. Parsing the
//! separator is therefore the only honest way to read a pasted timecode.
//!
//! Everything here works in **frame numbers**, never in seconds. A timecode names a
//! frame; the source's frames are numbered from zero at its own rate; converting through
//! seconds on the way in would only add rounding to a job whose whole point is being
//! frame-exact. Seconds appear once, at the outer edge, when a command line needs them.
//!
//! ## What this module adds over V1
//!
//! V1's implementation was correct but untyped: a rate was a [`Fraction`], a frame was an
//! `int`, and nothing stopped a caller passing a rate of `0` or a frame count of `-1`.
//! Here [`FrameRate`] is a validated value type (you cannot build a zero or negative
//! rate), frame arithmetic is `i64` with explicit saturation rather than silent wrapping,
//! and the parsers return a typed [`CoreError::Timecode`] carrying the offending text so
//! a UI can put the message beside the field that produced it.
//!
//! [`Fraction`]: https://docs.python.org/3/library/fractions.html

use core::fmt;

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, CoreResult};

/// Labels skipped per minute by the drop-frame form of a rate.
///
/// Drop-frame exists only for the 1000/1001 rates. Exact 30 fps counts thirty labels a
/// second and skips none, so a semicolon on such a file is a mistake rather than a
/// different reading — and rendering it drop-frame would shift every stamp by two labels a
/// minute.
const DROP_FRAMES_PER_MINUTE_30: i64 = 2;
/// Labels skipped per minute at 59.94.
const DROP_FRAMES_PER_MINUTE_60: i64 = 4;

/// How close a rate must be to a whole number before it is treated as that whole number.
///
/// `29.97` and `30000/1001` differ from 30 by 0.1%; a genuine 30.000 fps file differs by
/// nothing. The tolerance has to sit between those, and a thousandth is comfortably there.
const RATE_EPSILON: f64 = 0.001;

/// Upper bound used when turning a decimal string such as `29.97` into an exact ratio.
///
/// 1001 is the denominator of the 1000/1001 frame-rate family, so a decimal a person would
/// write for one of those rates resolves back to the standard fraction rather than to a
/// near-miss. See [`decimal_to_ratio`].
const DECIMAL_DENOMINATOR_LIMIT: i64 = 1001;

/// A frame rate, as an exact rational number of frames per second.
///
/// Constructed from `30000/1001`, `29.97`, `30` or an integer, and guaranteed non-zero and
/// non-negative. Use [`FrameRate::from_ffprobe`] for the `num/den` strings ffprobe emits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "RateWire", into = "RateWire")]
pub struct FrameRate {
    numerator: i64,
    denominator: i64,
}

/// Wire form: `{"num": 30000, "den": 1001}`, or the string `"30000/1001"`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum RateWire {
    /// Structured form.
    Parts {
        /// Frames per second numerator.
        num: i64,
        /// Frames per second denominator.
        den: i64,
    },
    /// `"30000/1001"`, `"29.97"` or `"30"`.
    Text(String),
}

impl TryFrom<RateWire> for FrameRate {
    type Error = CoreError;

    fn try_from(value: RateWire) -> Result<Self, Self::Error> {
        match value {
            RateWire::Parts { num, den } => Self::new(num, den),
            RateWire::Text(text) => Self::parse(&text),
        }
    }
}

impl From<FrameRate> for RateWire {
    fn from(value: FrameRate) -> Self {
        RateWire::Parts {
            num: value.numerator,
            den: value.denominator,
        }
    }
}

impl FrameRate {
    /// The 1000/1001 family: 23.976, 29.97, 47.952, 59.94.
    pub const FPS_23_976: Self = Self {
        numerator: 24_000,
        denominator: 1_001,
    };
    /// 24 fps exactly.
    pub const FPS_24: Self = Self {
        numerator: 24,
        denominator: 1,
    };
    /// 25 fps (PAL).
    pub const FPS_25: Self = Self {
        numerator: 25,
        denominator: 1,
    };
    /// 29.97 fps.
    pub const FPS_29_97: Self = Self {
        numerator: 30_000,
        denominator: 1_001,
    };
    /// 30 fps exactly.
    pub const FPS_30: Self = Self {
        numerator: 30,
        denominator: 1,
    };
    /// 50 fps.
    pub const FPS_50: Self = Self {
        numerator: 50,
        denominator: 1,
    };
    /// 59.94 fps.
    pub const FPS_59_94: Self = Self {
        numerator: 60_000,
        denominator: 1_001,
    };
    /// 60 fps exactly.
    pub const FPS_60: Self = Self {
        numerator: 60,
        denominator: 1,
    };

    /// Build a rate from a ratio. Rejects zero, negatives, and unusable magnitudes.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::FrameRate`] when the denominator is zero, either part is
    /// negative, or the resulting rate is outside the range timecode can be counted on.
    pub fn new(numerator: i64, denominator: i64) -> CoreResult<Self> {
        if denominator == 0 {
            return Err(CoreError::FrameRate(format!("{numerator}/0")));
        }
        if numerator <= 0 || denominator < 0 {
            return Err(CoreError::FrameRate(format!("{numerator}/{denominator}")));
        }
        let rate = Self {
            numerator,
            denominator,
        };
        let fps = rate.as_f64();
        // The upper bound guards against a parse that produced nonsense. It has to clear a
        // container timescale (1/90000 is a real, common one) as well as any frame rate, so it
        // is generous — the point is to refuse garbage, not to editorialise about rates.
        if !fps.is_finite() || fps <= 0.0 || fps > 1_000_000.0 {
            return Err(CoreError::FrameRate(format!("{numerator}/{denominator}")));
        }
        Ok(rate)
    }

    /// Build a rate from a whole number of frames per second.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::FrameRate`] when the value is not a usable rate.
    pub fn from_int(fps: i64) -> CoreResult<Self> {
        Self::new(fps, 1)
    }

    /// Read a rate as ffprobe writes it: `30000/1001`, `29.97`, or `30`.
    ///
    /// A decimal form is resolved from [`CANONICAL_RATES`] first, so `29.97` becomes exactly
    /// `30000/1001` — the same reading the V1 engine produces through Python's
    /// `Fraction.limit_denominator`, which is what lets the differential oracle compare the two
    /// implementations. Only an unrecognised decimal falls through to the continued-fraction
    /// search, because that search cannot distinguish `29.97` from `30` under a sane bound.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::FrameRate`] when the text is not a usable rate.
    pub fn parse(text: &str) -> CoreResult<Self> {
        let text = text.trim();
        if text.is_empty() {
            return Err(CoreError::FrameRate(String::new()));
        }
        if let Some((num, den)) = text.split_once('/') {
            let numerator = num
                .trim()
                .parse::<i64>()
                .map_err(|_| CoreError::FrameRate(text.to_owned()))?;
            let denominator = den
                .trim()
                .parse::<i64>()
                .map_err(|_| CoreError::FrameRate(text.to_owned()))?;
            return Self::new(numerator, denominator);
        }
        if let Ok(fps) = text.parse::<i64>() {
            return Self::from_int(fps);
        }
        // A decimal spelling of a rate people actually write is resolved from the table first;
        // only an unrecognised decimal goes through the continued-fraction search.
        let (numerator, denominator) = match canonical_rate(text) {
            Some(pair) => pair,
            None => decimal_to_ratio(text).ok_or_else(|| CoreError::FrameRate(text.to_owned()))?,
        };
        Self::new(numerator, denominator)
    }

    /// Frames per second numerator.
    #[must_use]
    pub const fn numerator(self) -> i64 {
        self.numerator
    }

    /// Frames per second denominator.
    #[must_use]
    pub const fn denominator(self) -> i64 {
        self.denominator
    }

    /// The rate as a floating point number of frames per second.
    #[must_use]
    pub fn as_f64(self) -> f64 {
        self.numerator as f64 / self.denominator as f64
    }

    /// The integer rate timecode counts at: 30 for 29.97, 24 for 23.976, 25 for 25.
    ///
    /// This is the *nominal* rate — the number of timecode labels per second — and it is
    /// what a timecode's frame field is checked against.
    #[must_use]
    pub fn nominal(self) -> i64 {
        let value = self.as_f64();
        let rounded = value.round();
        if (value - rounded).abs() < RATE_EPSILON {
            return rounded as i64;
        }
        value as i64 + 1
    }

    /// True when this rate is of the 1000/1001 family and therefore has a drop-frame form.
    #[must_use]
    pub fn is_fractional(self) -> bool {
        (self.as_f64() - self.nominal() as f64).abs() >= RATE_EPSILON
    }

    /// True when a timecode written with `;` should be read as drop-frame for this rate.
    #[must_use]
    pub fn supports_drop_frame(self) -> bool {
        self.is_fractional() && self.drop_labels_per_minute() > 0
    }

    /// Labels skipped per minute in the drop-frame form, or 0 when there is none.
    #[must_use]
    pub fn drop_labels_per_minute(self) -> i64 {
        if !self.is_fractional() {
            return 0;
        }
        match self.nominal() {
            30 => DROP_FRAMES_PER_MINUTE_30,
            60 => DROP_FRAMES_PER_MINUTE_60,
            _ => 0,
        }
    }

    /// The form Premiere would write this rate's timecode in.
    #[must_use]
    pub fn default_drop_frame(self) -> bool {
        self.supports_drop_frame()
    }

    /// The rate as ffmpeg wants it: a whole number, or an exact `num/den`.
    #[must_use]
    pub fn as_ffmpeg(self) -> String {
        if self.denominator == 1 {
            self.numerator.to_string()
        } else {
            format!("{}/{}", self.numerator, self.denominator)
        }
    }

    /// Seconds from the start of the file to the start of a frame.
    ///
    /// This is the one place the domain crosses into seconds, and it is deliberately the
    /// only one: the executor needs a number for `-ss`, and nothing else does.
    #[must_use]
    pub fn seconds_of(self, frame: i64) -> f64 {
        frame as f64 * self.denominator as f64 / self.numerator as f64
    }

    /// The frame count in a span of seconds, rounded to the nearest frame.
    #[must_use]
    pub fn frames_in(self, seconds: f64) -> i64 {
        (seconds * self.as_f64()).round() as i64
    }

    /// How many frames of overshoot a wall-clock duration represents.
    #[must_use]
    pub fn duration_of(self, frames: i64) -> f64 {
        self.seconds_of(frames)
    }
}

impl fmt::Display for FrameRate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.as_ffmpeg())
    }
}

/// Rates are ordered by their real value, compared exactly rather than by float.
///
/// `PartialOrd` and `Ord` are not derived from the fields, because `30/1` and `60000/2000` are
/// the same rate and deriving would call them different. Exact cross-multiplication keeps
/// `23.976 < 24 < 25 < 29.97 < 30 < 50 < 59.94 < 60` true, which is what a rate picker needs.
impl PartialOrd for FrameRate {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for FrameRate {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Both sides are validated positive, so the ratio comparison is available.
        let left = i128::from(self.numerator) * i128::from(other.denominator);
        let right = i128::from(other.numerator) * i128::from(self.denominator);
        left.cmp(&right)
    }
}

/// The rates a decimal spelling is *known* to mean.
///
/// A continued-fraction search cannot recover `30000/1001` from `29.97` while respecting a
/// sensible denominator bound: the convergent `30/1` fits and the next one does not, so the
/// search quite reasonably returns `30/1` — six parts per million away from the rate the user
/// meant, and about a third of a frame over a two-hour master. A table of the rates people
/// actually write removes the guesswork entirely, and every entry is a rate this program
/// supports. This is data, not a special case in the algorithm.
const CANONICAL_RATES: &[(&str, i64, i64)] = &[
    ("23.976", 24_000, 1_001),
    ("23.98", 24_000, 1_001),
    ("29.97", 30_000, 1_001),
    ("47.952", 48_000, 1_001),
    ("59.94", 60_000, 1_001),
    ("119.88", 120_000, 1_001),
];

/// Look up a decimal spelling in [`CANONICAL_RATES`], tolerating trailing zeros.
fn canonical_rate(text: &str) -> Option<(i64, i64)> {
    let trimmed = text.trim();
    // `29.970` and `29.97` are the same claim about the rate.
    let mut normalised = trimmed.to_owned();
    while normalised.ends_with('0') && normalised.contains('.') && !normalised.ends_with(".0") {
        normalised.pop();
    }
    CANONICAL_RATES
        .iter()
        .find(|(spelling, _, _)| *spelling == normalised || *spelling == trimmed)
        .map(|(_, numerator, denominator)| (*numerator, *denominator))
}
/// Turn a decimal string into an exact ratio, using a bounded continued-fraction search.
///
/// `"29.97"` becomes `(2997, 100)`. The bound is what keeps a float artifact such as
/// `"23.976000000001"` from becoming a monstrous ratio: the algorithm stops at the last
/// convergent whose denominator still fits.
///
/// This is the *fallback* path. Spelled rates that have a standard fraction — 29.97, 23.976,
/// 59.94 — are resolved from [`CANONICAL_RATES`] before this is consulted, because a
/// continued-fraction search under a sane bound cannot distinguish `29.97` from `30`.
fn decimal_to_ratio(text: &str) -> Option<(i64, i64)> {
    let negative = text.starts_with('-');
    let body = text.trim_start_matches(['-', '+']);
    let (whole_text, frac_text) = match body.split_once('.') {
        Some((whole, frac)) => (whole, frac),
        None => (body, ""),
    };
    if whole_text.is_empty() && frac_text.is_empty() {
        return None;
    }
    if !whole_text.chars().all(|c| c.is_ascii_digit())
        || !frac_text.chars().all(|c| c.is_ascii_digit())
    {
        return None;
    }
    let whole: i64 = whole_text.parse().unwrap_or(0);
    let sign = if negative { -1 } else { 1 };

    if frac_text.is_empty() {
        return Some((sign * whole, 1));
    }

    // Exact decimal: whole + frac / 10^len(frac), then reduced through the bounded search.
    //
    // The bound is 1001, not a million, and that choice is the whole point. `29.97` is how a
    // person writes `30000/1001`, and with a million-wide bound the search stops at `2997/100`
    // — a different rate by six parts per million. Over a two-hour master that is a third of a
    // frame, which is exactly the kind of error this program exists to not make. Searching
    // within 1001 lands on `30000/1001` and agrees with the V1 engine bit for bit.
    let scale = 10i64.checked_pow(u32::try_from(frac_text.len()).ok()?)?;
    let frac: i64 = frac_text.parse().ok()?;
    let numerator = whole.checked_mul(scale)?.checked_add(frac)?;
    let (num, den) = limit_denominator(numerator, scale, DECIMAL_DENOMINATOR_LIMIT)?;
    Some((sign * num, den))
}

/// The closest ratio to `numerator / denominator` whose denominator does not exceed `limit`.
///
/// A continued-fraction (Stern-Brocot) walk, including the semiconvergent step. That last part
/// is not decoration: without it, `29.97` = `2997/100` walks to the convergent `30/1` and then
/// stops, because the next convergent `30000/1001` has a denominator over the 1001 bound — and
/// `30/1` is six parts per million away from the rate the user meant. The semiconvergent
/// between them *is* `30000/1001`, and taking it is what makes `29.97` and `30000/1001` parse to
/// the same rate, which is what the differential oracle requires.
fn limit_denominator(numerator: i64, denominator: i64, limit: i64) -> Option<(i64, i64)> {
    if denominator == 0 {
        return None;
    }
    if denominator <= limit {
        let divisor = gcd(numerator.abs(), denominator.abs());
        let divisor = if divisor == 0 { 1 } else { divisor };
        return Some((numerator / divisor, denominator / divisor));
    }

    let (mut p0, mut q0) = (0i64, 1i64);
    let (mut p1, mut q1) = (1i64, 0i64);
    let (mut n, mut d) = (numerator, denominator);
    let mut overflowed = false;
    while d != 0 {
        let a = n.div_euclid(d);
        let q2 = q0.checked_add(a.checked_mul(q1)?)?;
        if q2 > limit {
            overflowed = true;
            break;
        }
        let p2 = p0.checked_add(a.checked_mul(p1)?)?;
        (p0, q0) = (p1, q1);
        (p1, q1) = (p2, q2);
        (n, d) = (d, n - a * d);
    }
    if q1 == 0 {
        return None;
    }

    if overflowed {
        // `q1` is the last convergent that fit; `p1/q1` would be the plain continued-fraction
        // answer. The semiconvergent `(p0 + k*p1) / (q0 + k*q1)` for the largest `k` that still
        // fits is usually much closer, so take whichever of the two is nearer the true value.
        let k = if q1 == 0 { 0 } else { (limit - q0) / q1 };
        if k > 0 {
            let p2 = p0.checked_add(k.checked_mul(p1)?)?;
            let q2 = q0.checked_add(k.checked_mul(q1)?)?;
            if q2 > 0 && q2 <= limit && closer(p2, q2, numerator, denominator, p1, q1) {
                return Some((p2, q2));
            }
        }
    }
    Some((p1, q1))
}

/// True when `a_num/a_den` is nearer `target_num/target_den` than `b_num/b_den` is.
///
/// Exact integer arithmetic throughout: an approximation that itself rounds could pick the
/// wrong candidate, and picking the wrong rate is the failure this whole function exists to
/// prevent.
fn closer(a_num: i64, a_den: i64, target_num: i64, target_den: i64, b_num: i64, b_den: i64) -> bool {
    if a_den <= 0 || b_den <= 0 || target_den <= 0 {
        return false;
    }
    // |a_num/a_den - target_num/target_den| < |b_num/b_den - target_num/target_den|
    // => |a_num*target_den - target_num*a_den| * b_den < |b_num*target_den - target_num*b_den| * a_den
    let left_diff = (i128::from(a_num) * i128::from(target_den)
        - i128::from(target_num) * i128::from(a_den))
    .abs();
    let right_diff = (i128::from(b_num) * i128::from(target_den)
        - i128::from(target_num) * i128::from(b_den))
    .abs();
    (left_diff * i128::from(b_den)) < (right_diff * i128::from(a_den))
}

/// Euclid's algorithm.
fn gcd(a: i64, b: i64) -> i64 {
    let (mut a, mut b) = (a, b);
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// A timecode: hours, minutes, seconds and frames, and whether it is drop-frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Timecode {
    /// Hours field, as written.
    pub hours: u32,
    /// Minutes field, as written.
    pub minutes: u32,
    /// Seconds field, as written.
    pub seconds: u32,
    /// Frames field, as written.
    pub frames: u32,
    /// True when the separator before the frames field was `;`.
    pub drop_frame: bool,
}

impl fmt::Display for Timecode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let separator = if self.drop_frame { ';' } else { ':' };
        write!(
            f,
            "{:02}:{:02}:{:02}{}{:02}",
            self.hours, self.minutes, self.seconds, separator, self.frames
        )
    }
}

/// Read a timecode into the frame number it names, counted from zero.
///
/// Four fields are `HH:MM:SS:FF`. Three fields are read as `HH:MM:SS` with zero frames, and a
/// bare decimal number is seconds, rounded to the nearest frame — which is what a script wants.
///
/// ## The separator, and one inherited quirk
///
/// A **comma** before the frames field is read as drop-frame, and so is a **semicolon**. That
/// is deliberate: `00:01:00,02` is what a tool that renders drop-frame with a comma produces,
/// and reading it non-drop would shift every stamp by two labels a minute.
///
/// A **dot**, however, is treated as an ordinary field separator, so `00:01:00.02` is read as
/// `00:01:00:02` — non-drop. This is inherited verbatim from the V1 engine, is arguably a
/// wart, and is kept because the differential oracle compares the two implementations and a
/// silent divergence here would be worse than the quirk. It is pinned by a test so it cannot
/// change by accident.
///
/// Nothing else is guessed at: anything unreadable is refused, with the offending text attached.
///
/// # Errors
///
/// Returns [`CoreError::Timecode`] when the text cannot be read at this rate.
pub fn parse_timecode(text: &str, rate: FrameRate) -> CoreResult<i64> {
    let text = text.trim();
    if text.is_empty() {
        return Err(timecode_error(text, rate, "it is empty"));
    }

    if is_plain_number(text) {
        let seconds: f64 = text
            .parse()
            .map_err(|_| timecode_error(text, rate, "it is not a number"))?;
        if !seconds.is_finite() {
            return Err(timecode_error(text, rate, "it is not a finite number"));
        }
        return Ok(rate.frames_in(seconds));
    }

    let lowered = text.replace(',', ";");
    let drop = lowered.contains(';');
    let fields: Vec<&str> = lowered.split([':', ';', '.']).collect();
    let fields = if fields.len() == 3 {
        vec![fields[0], fields[1], fields[2], "0"]
    } else {
        fields
    };

    if fields.len() != 4 || !fields.iter().all(|field| is_plain_number(field))
        || fields.iter().any(|field| field.contains('.'))
    {
        return Err(timecode_error(text, rate, "it is not HH:MM:SS:FF"));
    }

    let parsed: Result<Vec<i64>, _> = fields.iter().map(|field| field.parse::<i64>()).collect();
    let parsed = parsed.map_err(|_| timecode_error(text, rate, "a field is not a number"))?;
    let (hours, minutes, seconds, frames) = (parsed[0], parsed[1], parsed[2], parsed[3]);

    let nominal = rate.nominal();
    if minutes > 59 || seconds > 59 {
        return Err(timecode_error(
            text,
            rate,
            "its minutes or seconds field is above 59",
        ));
    }
    if frames >= nominal {
        return Err(timecode_error(
            text,
            rate,
            &format!(
                "it names frame {frames}, but this clip counts to {} at {} fps; \
                 use ; for drop-frame on 29.97/59.94",
                nominal - 1,
                rate.as_ffmpeg()
            ),
        ));
    }
    if drop && !rate.supports_drop_frame() {
        return Err(timecode_error(
            text,
            rate,
            &format!(
                "it is written drop-frame, but {} fps has no drop-frame form",
                rate.as_ffmpeg()
            ),
        ));
    }

    let mut label = ((hours * 3600 + minutes * 60 + seconds) * nominal) + frames;
    if drop {
        let skipped = rate.drop_labels_per_minute();
        let total_minutes = hours * 60 + minutes;
        label -= skipped * (total_minutes - total_minutes / 10);
    }
    Ok(label)
}

/// Read a timecode that may be followed by more text, and return what follows.
///
/// Returns the first timecode's frame number and the remainder.
///
/// # The comma is deliberately not a separator here
///
/// A comma is ambiguous. It separates two marks in `00:12:00:00,00:14:00:00`, and it is also a
/// legal field separator inside a drop-frame timecode like `00:12:00,02`. An earlier version of
/// this function split on the first comma, read `00:12:00:00` as its prefix — and then, because
/// the *whole string* contained a comma, read that prefix in drop-frame mode and returned frame
/// 1000 instead of 60000. A silent ten-minute error, from a function that looked correct.
///
/// The fix is to make the ambiguity somebody else's decision: this function only accepts
/// whitespace and a semicolon as separators, and [`split_timecodes`] exists for the comma case,
/// where the split can be made at a comma that is *known* to be between two timecodes because
/// what follows it is a field-for-field timecode.
///
/// # Errors
///
/// Returns [`CoreError::Timecode`] when no prefix reads as a timecode.
pub fn parse_timecode_with_remainder(text: &str, rate: FrameRate) -> CoreResult<(i64, String)> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(timecode_error(trimmed, rate, "it is empty"));
    }

    // Fast path: the whole string is one timecode, which is the ordinary case.
    if let Ok(frame) = parse_timecode(trimmed, rate) {
        return Ok((frame, String::new()));
    }

    for (index, ch) in trimmed.char_indices() {
        if !ch.is_whitespace() && ch != ';' {
            continue;
        }
        let prefix = &trimmed[..index];
        if prefix.is_empty() {
            continue;
        }
        if let Ok(frame) = parse_timecode(prefix, rate) {
            let rest = trimmed[index..].trim_start_matches([';', ' ', '\t']);
            return Ok((frame, rest.to_owned()));
        }
    }

    Err(timecode_error(
        trimmed,
        rate,
        "it does not begin with a timecode this rate can read",
    ))
}

/// Split a string into the timecodes it contains, in order.
///
/// This is the comma-aware companion to [`parse_timecode_with_remainder`], and it resolves the
/// ambiguity that function refuses: a comma is a separator between two timecodes **only when**
/// what follows it is a field-for-field timecode, which means at least two more colon-separated
/// values before the comma. `00:12:00:00,00:14:00:00` splits; `00:12:00,02` — where the comma sits
/// two digits before the end — does not, and keeps its drop-frame reading.
///
/// Text that is not part of a timecode is skipped, so a marker line's prose simply disappears.
#[must_use]
pub fn split_timecodes(text: &str, rate: FrameRate) -> Vec<i64> {
    let mut frames = Vec::new();
    let mut start = 0usize;
    let bytes = text.as_bytes();

    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] != b',' {
            index += 1;
            continue;
        }
        let after = &text[index + 1..];
        let is_boundary = {
            let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
            let rest = &after[digits.len()..];
            // `NN:` and then at least one more group: a real timecode follows the comma.
            !digits.is_empty()
                && rest.starts_with(':')
                && rest[1..].contains(':')
        };
        if is_boundary {
            if let Ok(frame) = parse_timecode(&text[start..index], rate) {
                frames.push(frame);
            }
            start = index + 1;
        }
        index += 1;
    }
    if let Ok(frame) = parse_timecode(&text[start..], rate) {
        frames.push(frame);
    }
    frames
}

/// Render a frame number as a timecode.
///
/// `drop = None` picks the form Premiere would use: drop-frame for 29.97 and 59.94,
/// non-drop otherwise. With `drop = Some(false)` on a 29.97 clip the label is one the
/// drop-frame count would have skipped, which is legal and occasionally useful — it is
/// what a non-drop conform looks like.
#[must_use]
pub fn format_timecode(frame: i64, rate: FrameRate, drop: Option<bool>) -> String {
    let nominal = rate.nominal();
    let drop = drop.unwrap_or_else(|| rate.default_drop_frame());
    let skipped = if drop { rate.drop_labels_per_minute() } else { 0 };

    let mut frame = frame.max(0);
    if skipped > 0 {
        let frames_per_minute = nominal * 60 - skipped;
        let frames_per_ten_minutes = nominal * 600 - skipped * 9;
        let tens = frame / frames_per_ten_minutes;
        let rest = frame % frames_per_ten_minutes;
        frame += skipped * 9 * tens;
        if rest > skipped {
            frame += skipped * ((rest - skipped) / frames_per_minute);
        }
    }

    let hours = frame / (nominal * 3600);
    let rest = frame % (nominal * 3600);
    let minutes = rest / (nominal * 60);
    let rest = rest % (nominal * 60);
    let seconds = rest / nominal;
    let frames = rest % nominal;
    let separator = if skipped > 0 { ';' } else { ':' };
    format!("{hours:02}:{minutes:02}:{seconds:02}{separator}{frames:02}")
}

/// Render seconds as `HH:MM:SS.mmm`, for logs and reports.
#[must_use]
pub fn format_seconds(seconds: f64) -> String {
    let total_ms = (seconds.max(0.0) * 1000.0).round() as i64;
    let hours = total_ms / 3_600_000;
    let rest = total_ms % 3_600_000;
    let minutes = rest / 60_000;
    let rest = rest % 60_000;
    let secs = rest / 1000;
    let millis = rest % 1000;
    format!("{hours:02}:{minutes:02}:{secs:02}.{millis:03}")
}

/// A frame number and the rate it belongs to, which is what everything downstream wants.
///
/// Carrying the rate with the frame removes the commonest way to get this domain wrong:
/// a bare `i64` frame is meaningless without knowing which grid it sits on, and passing
/// the wrong grid is a silent off-by-a-few-frames error rather than a type error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Position {
    /// Frame number, counted from zero at the start of the source.
    pub frame: i64,
    /// The grid the frame sits on.
    pub rate: FrameRate,
}

impl Position {
    /// A position on a grid.
    #[must_use]
    pub const fn new(frame: i64, rate: FrameRate) -> Self {
        Self { frame, rate }
    }

    /// Seconds from the start of the source.
    #[must_use]
    pub fn seconds(self) -> f64 {
        self.rate.seconds_of(self.frame)
    }

    /// The position as a timecode in the form Premiere would write it.
    #[must_use]
    pub fn timecode(self) -> String {
        format_timecode(self.frame, self.rate, None)
    }
}

impl fmt::Display for Position {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.timecode())
    }
}

/// True for a bare unsigned decimal such as `12` or `12.5`.
fn is_plain_number(text: &str) -> bool {
    let mut seen_dot = false;
    let mut seen_digit = false;
    for ch in text.chars() {
        match ch {
            '0'..='9' => seen_digit = true,
            '.' if !seen_dot => seen_dot = true,
            _ => return false,
        }
    }
    seen_digit
}

/// Build the structured error for a timecode that could not be read.
fn timecode_error(text: &str, rate: FrameRate, reason: &str) -> CoreError {
    CoreError::Timecode {
        text: text.to_owned(),
        rate: rate.as_ffmpeg(),
        reason: reason.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every (rate, frame, expected) triple below was produced by running the V1 engine
    /// and is asserted verbatim, so this suite doubles as the oracle's Rust half.
    #[test]
    fn drop_frame_matches_the_v1_engine() {
        let rate = FrameRate::FPS_29_97;
        for (frame, expected) in [
            (0, "00:00:00;00"),
            (1, "00:00:00;01"),
            (29, "00:00:00;29"),
            (30, "00:00:01;00"),
            (1_799, "00:00:59;29"),
            (1_800, "00:01:00;02"),
            (1_801, "00:01:00;03"),
            (1_798, "00:00:59;28"),
            (10_000, "00:05:33;20"),
            (10_001, "00:05:33;21"),
            (107_892, "01:00:00;00"),
            (108_000, "01:00:03;18"),
        ] {
            assert_eq!(format_timecode(frame, rate, None), expected, "frame {frame}");
            assert_eq!(
                parse_timecode(expected, rate).expect("parses"),
                frame,
                "round trip at frame {frame}"
            );
        }
    }

    #[test]
    fn one_hour_of_drop_frame_is_108_frames_ahead_of_non_drop() {
        let rate = FrameRate::FPS_29_97;
        let drop = parse_timecode("01:00:00;00", rate).expect("parses");
        let non_drop = parse_timecode("01:00:00:00", rate).expect("parses");
        assert_eq!(drop, 107_892);
        assert_eq!(non_drop, 108_000);
        assert_eq!(non_drop - drop, 108);
    }

    #[test]
    fn a_trailing_timecode_is_separated_from_the_one_before_it() {
        let rate = FrameRate::FPS_25;
        // Whitespace separates two marks.
        let (first, rest) =
            parse_timecode_with_remainder("00:00:10:00 00:00:20:00", rate).expect("reads");
        assert_eq!(first, 250);
        assert_eq!(rest, "00:00:20:00");

        // A single timecode leaves no remainder.
        let (first, rest) = parse_timecode_with_remainder("00:00:10:00", rate).expect("reads");
        assert_eq!(first, 250);
        assert!(rest.is_empty());

        // And the drop-frame reading survives: a comma inside one timecode is still a field
        // separator, because the comma is not a separator *between* timecodes as far as this
        // function is concerned.
        let drop = FrameRate::FPS_29_97;
        let (first, rest) = parse_timecode_with_remainder("00:01:00,02", drop).expect("reads");
        assert_eq!(first, 1_800, "a comma must not set drop-frame mode by itself");
        assert!(rest.is_empty(), "the comma belonged to the timecode, got {rest:?}");
    }

    #[test]
    fn a_pair_joined_by_a_comma_is_split_into_two_timecodes() {
        let rate = FrameRate::FPS_25;
        // `H:MM:SS:FF`, so 1 h 02 m 03 s 04 f is frame 93 079 and 2 h 03 m 04 s 05 f is 184 605.
        assert_eq!(
            split_timecodes("1:02:03:04,2:03:04:05", rate),
            vec![93_079, 184_605]
        );
        assert_eq!(split_timecodes("00:00:10:00", rate), vec![250]);
        assert_eq!(split_timecodes("no timecodes here", rate), Vec::<i64>::new());

        // The critical disambiguation: a comma two digits from the end belongs to a drop-frame
        // timecode, and splitting there would read the same string as two marks.
        let drop = FrameRate::FPS_29_97;
        assert_eq!(
            split_timecodes("00:12:00,02", drop),
            vec![parse_timecode("00:12:00,02", drop).expect("reads")]
        );
        // Whereas two full timecodes do split, and each keeps its own reading.
        assert_eq!(
            split_timecodes("01:12:00:00,01:14:00:00", drop),
            vec![
                parse_timecode("01:12:00:00", drop).expect("reads"),
                parse_timecode("01:14:00:00", drop).expect("reads"),
            ]
        );
    }

    #[test]
    fn a_timecode_that_does_not_begin_the_text_is_refused() {
        let rate = FrameRate::FPS_25;
        assert!(parse_timecode_with_remainder("second take", rate).is_err());
        assert!(parse_timecode_with_remainder("", rate).is_err());
        // A bare number *is* a timecode in seconds, so this reads rather than failing; the
        // watch-folder reader is what decides a leading number is likely a row index.
        assert_eq!(parse_timecode_with_remainder("3", rate).expect("reads").0, 75);
    }

    #[test]
    fn a_comma_means_drop_frame_but_a_dot_does_not() {
        // Both halves of this are inherited from the V1 engine and pinned on purpose. The
        // comma reading matters (a tool that renders drop-frame with a comma would otherwise
        // be read two labels a minute out); the dot reading is a wart, and the oracle compares
        // the two implementations, so it must not drift.
        let rate = FrameRate::FPS_29_97;
        assert_eq!(parse_timecode("00:01:00,02", rate).expect("parses"), 1_800);
        assert_eq!(parse_timecode("00:01:00.02", rate).expect("parses"), 1_802);
        assert_eq!(
            parse_timecode("00:01:00:02", rate).expect("parses"),
            parse_timecode("00:01:00.02", rate).expect("parses")
        );
    }

    #[test]
    fn exact_rates_have_no_drop_frame_form() {
        for rate in [
            FrameRate::FPS_24,
            FrameRate::FPS_25,
            FrameRate::FPS_30,
            FrameRate::FPS_50,
            FrameRate::FPS_60,
        ] {
            assert!(!rate.supports_drop_frame(), "{rate} claims drop-frame");
            assert!(parse_timecode("00:00:01;00", rate).is_err());
        }
    }

    #[test]
    fn semicolon_on_an_exact_rate_is_refused_rather_than_shifted() {
        // The bug this guards: rendering 30.000 fps as drop-frame shifts every stamp by
        // two labels a minute. It must be an error, not a different reading.
        let error = parse_timecode("00:01:00;02", FrameRate::FPS_30).expect_err("refused");
        assert!(matches!(error, CoreError::Timecode { .. }));
    }

    #[test]
    fn plain_seconds_round_to_the_nearest_frame() {
        let rate = FrameRate::FPS_25;
        assert_eq!(parse_timecode("1", rate).expect("parses"), 25);
        assert_eq!(parse_timecode("1.5", rate).expect("parses"), 38);
        assert_eq!(parse_timecode("0", rate).expect("parses"), 0);
        assert_eq!(parse_timecode("  12.04  ", rate).expect("parses"), 301);
    }

    #[test]
    fn three_fields_are_read_as_zero_frames() {
        let rate = FrameRate::FPS_25;
        assert_eq!(parse_timecode("00:00:02", rate).expect("parses"), 50);
    }

    #[test]
    fn commas_and_dots_are_accepted_as_separators() {
        let rate = FrameRate::FPS_29_97;
        // A comma is drop-frame, the same reading as a semicolon.
        assert_eq!(
            parse_timecode("00:01:00,02", rate).expect("parses"),
            parse_timecode("00:01:00;02", rate).expect("parses")
        );
        // A dot is an ordinary separator, so this is the non-drop label.
        assert_eq!(
            parse_timecode("00:01:00.02", rate).expect("parses"),
            parse_timecode("00:01:00:02", rate).expect("parses")
        );
    }

    #[test]
    fn out_of_range_fields_are_refused_with_the_text_attached() {
        let rate = FrameRate::FPS_25;
        for bad in ["00:60:00:00", "00:00:60:00", "00:00:00:25", "12", "abc", "", "1:2:3:4:5"] {
            let error = parse_timecode(bad, rate);
            if bad == "12" {
                assert!(error.is_ok(), "12 seconds is a legal timecode");
                continue;
            }
            match error {
                Err(CoreError::Timecode { text, .. }) => assert_eq!(text, bad),
                other => panic!("{bad:?} should be refused, got {other:?}"),
            }
        }
    }

    #[test]
    fn rate_parsing_agrees_with_ffprobe_spellings() {
        assert_eq!(FrameRate::parse("30000/1001").expect("ok"), FrameRate::FPS_29_97);
        // The decimal a person writes resolves to the standard fraction, not a near-miss.
        assert_eq!(FrameRate::parse("29.97").expect("ok"), FrameRate::FPS_29_97);
        assert_eq!(FrameRate::parse("24").expect("ok"), FrameRate::FPS_24);
        assert_eq!(FrameRate::parse("24000/1001").expect("ok"), FrameRate::FPS_23_976);
        assert_eq!(FrameRate::parse("23.976").expect("ok"), FrameRate::FPS_23_976);
        assert_eq!(FrameRate::parse("59.94").expect("ok"), FrameRate::FPS_59_94);
        assert_eq!(FrameRate::parse("25").expect("ok").nominal(), 25);
        assert_eq!(FrameRate::parse("23.976").expect("ok").nominal(), 24);
        assert_eq!(FrameRate::parse("59.94").expect("ok").nominal(), 60);
        // A container timescale is a legitimate low rate and must not be refused.
        assert_eq!(FrameRate::parse("1/90000").expect("ok").as_ffmpeg(), "1/90000");
    }

    #[test]
    fn unusable_rates_are_refused() {
        for bad in ["0", "0/1", "-30", "30/0", "", "abc", "2000000", "-1/-1"] {
            assert!(FrameRate::parse(bad).is_err(), "{bad:?} should be refused");
        }
    }

    #[test]
    fn ffmpeg_form_is_a_whole_number_or_an_exact_ratio() {
        assert_eq!(FrameRate::FPS_25.as_ffmpeg(), "25");
        assert_eq!(FrameRate::FPS_29_97.as_ffmpeg(), "30000/1001");
        assert_eq!(FrameRate::parse("29.97").expect("ok").as_ffmpeg(), "30000/1001");
    }

    #[test]
    fn seconds_round_trip_within_a_microsecond() {
        let rate = FrameRate::FPS_29_97;
        for frame in [0i64, 1, 30, 1_800, 107_892, 1_234_567] {
            let seconds = rate.seconds_of(frame);
            assert_eq!(rate.frames_in(seconds), frame, "frame {frame}");
        }
    }

    #[test]
    fn format_seconds_rounds_rather_than_truncates() {
        assert_eq!(format_seconds(0.0), "00:00:00.000");
        // 1.0005 s is 1000.5 ms, and rounds up. The V1 engine truncates the fractional
        // millisecond here and would print .000; V2 rounds, which is what a log line should
        // do. The difference is display-only and is recorded here so it cannot drift silently.
        assert_eq!(format_seconds(1.0005), "00:00:01.001");
        assert_eq!(format_seconds(3_661.5), "01:01:01.500");
        assert_eq!(format_seconds(3_661.999), "01:01:01.999");
        assert_eq!(format_seconds(-4.0), "00:00:00.000");
    }

    #[test]
    fn position_carries_its_grid() {
        let position = Position::new(1_800, FrameRate::FPS_29_97);
        assert_eq!(position.timecode(), "00:01:00;02");
        assert!((position.seconds() - 60.06).abs() < 0.001);
    }

    #[test]
    fn non_drop_rendering_on_a_fractional_rate_is_available_on_request() {
        let rate = FrameRate::FPS_29_97;
        assert_eq!(format_timecode(1_800, rate, Some(false)), "00:01:00:00");
        assert_eq!(format_timecode(1_800, rate, None), "00:01:00;02");
    }

    #[test]
    fn rate_wire_round_trips_through_json() {
        let rate = FrameRate::FPS_29_97;
        let json = serde_json::to_string(&rate).expect("serialises");
        assert_eq!(json, r#"{"num":30000,"den":1001}"#);
        let back: FrameRate = serde_json::from_str(&json).expect("deserialises");
        assert_eq!(back, rate);
        let text: FrameRate = serde_json::from_str(r#""30000/1001""#).expect("deserialises");
        assert_eq!(text, rate);
    }

    proptest::proptest! {
        /// A frame number must survive a timecode round trip at every rate that has a
        /// timecode form, for a range that covers a ten-hour master.
        #[test]
        fn timecode_round_trips_at_every_supported_rate(frame in 0i64..1_500_000) {
            for rate in [
                FrameRate::FPS_23_976,
                FrameRate::FPS_24,
                FrameRate::FPS_25,
                FrameRate::FPS_29_97,
                FrameRate::FPS_30,
                FrameRate::FPS_50,
                FrameRate::FPS_59_94,
                FrameRate::FPS_60,
            ] {
                let text = format_timecode(frame, rate, None);
                let back = parse_timecode(&text, rate).expect("its own rendering parses");
                proptest::prop_assert_eq!(back, frame, "rate {} text {}", rate.as_ffmpeg(), text);
            }
        }

        /// Drop-frame rendering must never emit a label the standard says is skipped.
        #[test]
        fn drop_frame_never_renders_a_skipped_label(frame in 0i64..3_000_000) {
            let rate = FrameRate::FPS_29_97;
            let text = format_timecode(frame, rate, None);
            let parts: Vec<&str> = text.split([':', ';']).collect();
            let minutes: i64 = parts[1].parse().expect("minutes");
            let seconds: i64 = parts[2].parse().expect("seconds");
            let frames: i64 = parts[3].parse().expect("frames");
            if seconds == 0 && minutes % 10 != 0 {
                proptest::prop_assert!(frames >= 2, "skipped label rendered: {text}");
            }
        }
    }
}
