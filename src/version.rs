use anyhow::anyhow;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    major: u64,
    minor: u64,
    patch: u64,
    rc: ReleaseCandidate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseCandidate {
    rc: String,
    num: u64,
}

impl Version {
    /// Hand-written cursor parser for the project's own format
    /// `major.minor.patch[-rcN]` (pre-release must be `rc` + digits, e.g.
    /// `2.1.3-rc1`), optionally prefixed with `v`. Anything else is rejected.
    pub fn parse(version: &str) -> anyhow::Result<Self> {
        if version.is_empty() {
            return Err(anyhow!("version is empty"));
        }

        let bytes = version.as_bytes();
        let mut pos = 0usize;

        // Optional leading 'v', e.g. "v1.2.3"
        if bytes.first() == Some(&b'v') {
            pos += 1;
        }

        let major = Self::read_number(bytes, &mut pos)?;
        Self::expect_dot(bytes, &mut pos)?;
        let minor = Self::read_number(bytes, &mut pos)?;
        Self::expect_dot(bytes, &mut pos)?;
        let patch = Self::read_number(bytes, &mut pos)?;

        // Optional release candidate, e.g. "-rc.1" / "-rc2"
        let rc = if bytes.get(pos) == Some(&b'-') {
            pos += 1;
            Self::read_release_candidate(&version[pos..])?
        } else {
            ReleaseCandidate {
                rc: String::new(),
                num: 0,
            }
        };

        Ok(Version {
            major,
            minor,
            patch,
            rc,
        })
    }

    /// True when a pre-release / release-candidate suffix is present.
    pub fn is_prerelease(&self) -> bool {
        !self.rc.rc.is_empty()
    }

    /// Sort key: numeric core first, a release outranks a pre-release,
    /// then the pre-release number and name break ties.
    fn key(&self) -> (u64, u64, u64, u8, u64, &str) {
        let rank = if self.rc.rc.is_empty() { 1 } else { 0 };
        (
            self.major,
            self.minor,
            self.patch,
            rank,
            self.rc.num,
            self.rc.rc.as_str(),
        )
    }

    /// Reads a run of ASCII digits starting at `*pos`, accumulating the value
    /// with `value * 10 + digit`. Advances `*pos` past the digits it consumed.
    fn read_number(bytes: &[u8], pos: &mut usize) -> anyhow::Result<u64> {
        let start = *pos;
        let mut value: u64 = 0;

        while let Some(&digit) = bytes.get(*pos) {
            if !digit.is_ascii_digit() {
                break;
            }
            // Guard against overflow instead of silently wrapping / panicking.
            value = value
                .checked_mul(10)
                .and_then(|v| v.checked_add((digit - b'0') as u64))
                .ok_or_else(|| anyhow!("version number overflow"))?;
            *pos += 1;
        }

        if *pos == start {
            return Err(anyhow!("expected a number at position {start}"));
        }
        // Reject leading zeros like "01", but allow the single "0".
        if *pos - start > 1 && bytes[start] == b'0' {
            return Err(anyhow!("leading zero is not allowed at position {start}"));
        }
        Ok(value)
    }

    /// Consumes a single '.' separator, advancing the cursor.
    fn expect_dot(bytes: &[u8], pos: &mut usize) -> anyhow::Result<()> {
        match bytes.get(*pos) {
            Some(&b'.') => {
                *pos += 1;
                Ok(())
            }
            Some(&c) => Err(anyhow!(
                "expected '.' at position {pos}, found '{}'",
                c as char
            )),
            None => Err(anyhow!("unexpected end, expected '.'")),
        }
    }

    /// Parses the pre-release suffix, accepting only `rc` followed by digits,
    /// e.g. "rc1" -> { rc: "rc1", num: 1 }. All other spellings are rejected.
    fn read_release_candidate(text: &str) -> anyhow::Result<ReleaseCandidate> {
        // Must start with the literal "rc".
        let digits = text
            .strip_prefix("rc")
            .ok_or_else(|| anyhow!("only 'rc<number>' pre-release is supported, found '{text}'"))?;
        // "rc" alone is invalid; the rest must be all ASCII digits.
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return Err(anyhow!("invalid release candidate number in '{text}'"));
        }
        let num: u64 = digits
            .parse()
            .map_err(|_| anyhow!("release candidate number overflow in '{text}'"))?;
        Ok(ReleaseCandidate {
            rc: text.to_string(),
            num,
        })
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.key().cmp(&other.key())
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.rc.rc.is_empty() {
            write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
        } else {
            write!(
                f,
                "{}.{}.{}-{}",
                self.major, self.minor, self.patch, self.rc.rc
            )
        }
    }
}
