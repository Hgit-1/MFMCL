//! Bounded, three-valued predicates. Fabric caret means same major (including
//! major zero), not npm's left-most-nonzero rule. No npm prerelease exclusion is
//! applied: Fabric compares prereleases directly. Unrecognized grammar is unknown.
use serde_json::Value;
use std::cmp::Ordering;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Match {
    Yes,
    No,
    Unknown,
}

impl Match {
    pub(super) fn any(items: impl IntoIterator<Item = Self>) -> Self {
        let mut unknown = false;
        for item in items {
            match item {
                Self::Yes => return Self::Yes,
                Self::Unknown => unknown = true,
                Self::No => {}
            }
        }
        if unknown { Self::Unknown } else { Self::No }
    }
    fn all(items: impl IntoIterator<Item = Self>) -> Self {
        let mut unknown = false;
        for item in items {
            match item {
                Self::No => return Self::No,
                Self::Unknown => unknown = true,
                Self::Yes => {}
            }
        }
        if unknown { Self::Unknown } else { Self::Yes }
    }
    fn from_bool(value: bool) -> Self {
        if value { Self::Yes } else { Self::No }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Semantic {
    core: Vec<u64>,
    pre: Option<Vec<String>>,
}

fn semantic(text: &str) -> Option<Semantic> {
    if text.is_empty() || text.len() > 256 {
        return None;
    }
    let (text, build) = text
        .split_once('+')
        .map_or((text, None), |(a, b)| (a, Some(b)));
    let identifiers = |s: &str| {
        !s.is_empty()
            && s.split('.')
                .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
    };
    if build.is_some_and(|s| !identifiers(s)) {
        return None;
    }
    let (core, pre) = text
        .split_once('-')
        .map_or((text, None), |(a, b)| (a, Some(b)));
    // An empty prerelease is Fabric's lowest prerelease sentinel (e.g. 1.2-).
    if pre.is_some_and(|s| !s.is_empty() && !identifiers(s)) {
        return None;
    }
    let core: Vec<_> = core
        .split('.')
        .map(|p| {
            if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
                None
            } else {
                p.parse::<u64>().ok()
            }
        })
        .collect::<Option<_>>()?;
    if core.len() > 16 {
        return None;
    }
    Some(Semantic {
        core,
        pre: pre.map(|p| p.split('.').map(str::to_owned).collect()),
    })
}

impl Semantic {
    fn component(&self, index: usize) -> u64 {
        self.core.get(index).copied().unwrap_or(0)
    }
    fn compare(&self, other: &Self) -> Ordering {
        for i in 0..self.core.len().max(other.core.len()) {
            let order = self.component(i).cmp(&other.component(i));
            if order != Ordering::Equal {
                return order;
            }
        }
        match (&self.pre, &other.pre) {
            (None, None) => Ordering::Equal,
            (None, Some(_)) => Ordering::Greater,
            (Some(_), None) => Ordering::Less,
            (Some(a), Some(b)) => {
                for (a, b) in a.iter().zip(b) {
                    let numeric = |s: &str| !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit());
                    let order = match (numeric(a), numeric(b)) {
                        (true, true) => {
                            let a = a.trim_start_matches('0');
                            let b = b.trim_start_matches('0');
                            a.len().cmp(&b.len()).then_with(|| a.cmp(b))
                        }
                        (true, false) => Ordering::Less,
                        (false, true) => Ordering::Greater,
                        _ => a.cmp(b),
                    };
                    if order != Ordering::Equal {
                        return order;
                    }
                }
                a.len().cmp(&b.len())
            }
        }
    }
}

pub(super) fn compare_semantic(a: &str, b: &str) -> Option<Ordering> {
    Some(semantic(a)?.compare(&semantic(b)?))
}

#[derive(Clone)]
enum Term {
    Any,
    Compare(String, Semantic),
    Wildcard(Vec<u64>),
    Literal(String),
}

fn term(text: &str) -> Option<Term> {
    if matches!(text, "*" | "x" | "X") {
        return Some(Term::Any);
    }
    let (op, text) = [">=", "<=", ">", "<", "=", "~", "^"]
        .into_iter()
        .find_map(|op| text.strip_prefix(op).map(|rest| (op, rest)))
        .unwrap_or(("=", text));
    if text.is_empty() {
        return None;
    }
    if text.split('.').any(|p| matches!(p, "*" | "x" | "X")) {
        if op != "=" || text.contains(['-', '+']) {
            return None;
        }
        let mut prefix = Vec::new();
        let mut wildcard = false;
        for part in text.split('.') {
            if matches!(part, "*" | "x" | "X") {
                wildcard = true;
            } else if wildcard || part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            } else {
                prefix.push(part.parse().ok()?);
            }
        }
        return (prefix.len() < 16).then_some(Term::Wildcard(prefix));
    }
    if let Some(version) = semantic(text) {
        return Some(Term::Compare(op.into(), version));
    }
    // Fabric accepts arbitrary string versions for equality, not range syntax.
    if op == "="
        && text.bytes().any(|b| b.is_ascii_alphanumeric())
        && text.len() <= 256
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b'+'))
    {
        Some(Term::Literal(text.into()))
    } else {
        None
    }
}

fn test_term(term: &Term, version: &str) -> Match {
    match term {
        Term::Any => Match::Yes,
        Term::Literal(literal) => Match::from_bool(literal == version),
        Term::Wildcard(prefix) => {
            let Some(v) = semantic(version) else {
                return Match::Unknown;
            };
            Match::from_bool(prefix.iter().enumerate().all(|(i, c)| v.component(i) == *c))
        }
        Term::Compare(op, required) => {
            let Some(actual) = semantic(version) else {
                return Match::Unknown;
            };
            let cmp = actual.compare(required);
            Match::from_bool(match op.as_str() {
                "=" => cmp == Ordering::Equal,
                ">" => cmp == Ordering::Greater,
                ">=" => cmp != Ordering::Less,
                "<" => cmp == Ordering::Less,
                "<=" => cmp != Ordering::Greater,
                "^" => cmp != Ordering::Less && actual.component(0) == required.component(0),
                "~" => {
                    cmp != Ordering::Less
                        && actual.component(0) == required.component(0)
                        && actual.component(1) == required.component(1)
                }
                _ => return Match::Unknown,
            })
        }
    }
}

fn fabric(text: &str, version: &str) -> Match {
    if text.len() > 4096 {
        return Match::Unknown;
    }
    if text.trim().is_empty() {
        return Match::Yes;
    }
    let mut alternatives = Vec::new();
    for clause in text.split("||") {
        let tokens: Vec<_> = clause.split_whitespace().collect();
        if tokens.is_empty() || tokens.len() > 128 {
            return Match::Unknown;
        }
        let mut terms = Vec::new();
        let mut i = 0;
        while i < tokens.len() {
            if tokens[i] == "&&" {
                return Match::Unknown;
            }
            let joined;
            let token = if matches!(tokens[i], ">=" | "<=" | ">" | "<" | "=" | "~" | "^") {
                i += 1;
                let Some(next) = tokens.get(i) else {
                    return Match::Unknown;
                };
                joined = format!("{}{next}", tokens[i - 1]);
                &joined
            } else {
                tokens[i]
            };
            let Some(parsed) = term(token) else {
                return Match::Unknown;
            };
            terms.push(parsed);
            i += 1;
            if tokens.get(i) == Some(&"&&") {
                i += 1;
                if i == tokens.len() {
                    return Match::Unknown;
                }
            }
        }
        alternatives.push(Match::all(terms.iter().map(|t| test_term(t, version))));
        if alternatives.len() > 128 {
            return Match::Unknown;
        }
    }
    Match::any(alternatives)
}

// Maven's common numeric/qualifier subset. Unknown qualifiers and exotic nested
// ComparableVersion list forms remain unknown rather than using lexical order.
fn maven_compare(a: &str, b: &str) -> Option<Ordering> {
    fn parts(s: &str) -> Option<(Vec<u64>, i8, u64)> {
        if s.len() > 256 {
            return None;
        }
        let s = s.to_ascii_lowercase();
        let split = s.bytes().position(|c| c.is_ascii_alphabetic());
        let (numbers, suffix) = split.map_or((s.as_str(), ""), |i| (&s[..i], &s[i..]));
        let numbers = numbers.trim_end_matches(['.', '-']);
        let core = numbers
            .split('.')
            .map(|p| {
                if p.is_empty() || !p.bytes().all(|c| c.is_ascii_digit()) {
                    None
                } else {
                    p.parse().ok()
                }
            })
            .collect::<Option<Vec<u64>>>()?;
        if core.len() > 16 {
            return None;
        }
        let pos = suffix
            .bytes()
            .position(|c| c.is_ascii_digit())
            .unwrap_or(suffix.len());
        let (qualifier, serial) = suffix.split_at(pos);
        let qualifier = qualifier.trim_end_matches(['.', '-']);
        let rank = match qualifier {
            "alpha" | "a" => -5,
            "beta" | "b" => -4,
            "milestone" | "m" => -3,
            "rc" | "cr" => -2,
            "snapshot" => -1,
            "" | "final" | "ga" | "release" => 0,
            "sp" => 1,
            _ => return None,
        };
        let serial = if serial.is_empty() {
            0
        } else {
            serial.parse().ok()?
        };
        Some((core, rank, serial))
    }
    let (a, aq, an) = parts(a)?;
    let (b, bq, bn) = parts(b)?;
    for i in 0..a.len().max(b.len()) {
        let order = a.get(i).unwrap_or(&0).cmp(b.get(i).unwrap_or(&0));
        if order != Ordering::Equal {
            return Some(order);
        }
    }
    Some(aq.cmp(&bq).then(an.cmp(&bn)))
}

pub(super) fn compare_maven(a: &str, b: &str) -> Option<Ordering> {
    maven_compare(a, b)
}

fn maven(text: &str, version: &str) -> Match {
    if text == "*" || text.is_empty() {
        return Match::Yes;
    }
    if text.len() > 4096 {
        return Match::Unknown;
    }
    if !text.starts_with(['[', '(']) {
        // Bare Maven versions are recommendations, not strict constraints.
        return if maven_compare(text, text).is_some() {
            Match::Yes
        } else {
            Match::Unknown
        };
    }
    let mut rest = text;
    let mut ranges = Vec::new();
    while !rest.is_empty() {
        if ranges.len() >= 128 || !rest.starts_with(['[', '(']) {
            return Match::Unknown;
        }
        let Some(end) = rest.find([']', ')']) else {
            return Match::Unknown;
        };
        let start_inclusive = rest.starts_with('[');
        let end_inclusive = rest.as_bytes()[end] == b']';
        let body = &rest[1..end];
        let result = if let Some((low, high)) = body.split_once(',') {
            let low = low.trim();
            let high = high.trim();
            if (!low.is_empty() && maven_compare(low, low).is_none())
                || (!high.is_empty() && maven_compare(high, high).is_none())
            {
                return Match::Unknown;
            }
            if high.contains(',')
                || (low.is_empty() && start_inclusive)
                || (high.is_empty() && end_inclusive)
            {
                return Match::Unknown;
            }
            if !low.is_empty() && !high.is_empty() {
                match maven_compare(low, high) {
                    Some(Ordering::Greater) | None => return Match::Unknown,
                    _ => {}
                }
            }
            let bound = |value: &str, lower: bool, inclusive: bool| {
                if value.is_empty() {
                    return Match::Yes;
                }
                let Some(cmp) = maven_compare(version, value) else {
                    return Match::Unknown;
                };
                Match::from_bool(
                    (inclusive && cmp == Ordering::Equal)
                        || cmp
                            == if lower {
                                Ordering::Greater
                            } else {
                                Ordering::Less
                            },
                )
            };
            Match::all([
                bound(low, true, start_inclusive),
                bound(high, false, end_inclusive),
            ])
        } else {
            if !start_inclusive || !end_inclusive || body.is_empty() {
                return Match::Unknown;
            }
            maven_compare(version, body)
                .map(|cmp| Match::from_bool(cmp == Ordering::Equal))
                .unwrap_or(Match::Unknown)
        };
        ranges.push(result);
        rest = &rest[end + 1..];
        if !rest.is_empty() {
            let Some(next) = rest.strip_prefix(',') else {
                return Match::Unknown;
            };
            rest = next.trim_start();
            if rest.is_empty() {
                return Match::Unknown;
            }
        }
    }
    Match::any(ranges)
}

pub(super) fn preference(a: &str, b: &str, forge: bool) -> Ordering {
    let compare = |a: &str, b: &str| {
        if forge {
            compare_maven(a, b)
        } else {
            compare_semantic(a, b)
        }
    };
    match (compare(a, a).is_some(), compare(b, b).is_some()) {
        (true, true) => compare(a, b).unwrap_or(Ordering::Equal),
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        (false, false) => a.cmp(b),
    }
}

pub(super) fn evaluate(predicate: &Value, version: &str, forge: bool) -> Match {
    fn inner(predicate: &Value, version: &str, forge: bool, depth: usize) -> Match {
        if depth > 16 {
            return Match::Unknown;
        }
        match predicate {
            Value::String(text) => {
                if forge {
                    maven(text.trim(), version)
                } else {
                    fabric(text, version)
                }
            }
            Value::Array(items) if !items.is_empty() && items.len() <= 128 => {
                Match::any(items.iter().map(|p| inner(p, version, forge, depth + 1)))
            }
            Value::Object(map) if !forge && map.len() == 1 => {
                let (key, value) = map.iter().next().unwrap();
                let Some(items) = value.as_array().filter(|v| !v.is_empty() && v.len() <= 128)
                else {
                    return Match::Unknown;
                };
                let values = items.iter().map(|p| inner(p, version, forge, depth + 1));
                match key.as_str() {
                    "any" => Match::any(values),
                    "all" => Match::all(values),
                    _ => Match::Unknown,
                }
            }
            _ => Match::Unknown,
        }
    }
    inner(predicate, version, forge, 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn fabric_ranges_follow_loader_not_npm_semantics() {
        for (predicate, version, expected) in [
            (">=1.2 <2", "1.9.0", Match::Yes),
            (">=1.2 <2", "2.0", Match::No),
            ("<1 || >=2", "2.1", Match::Yes),
            (">= 1 && < 2", "1.4", Match::Yes),
            ("1.2.x", "1.2.9-beta", Match::Yes),
            ("1.2.*", "1.3.0-alpha", Match::No),
            ("1.x", "1.0-alpha", Match::Yes),
            ("^0.2.3", "0.9", Match::Yes),
            ("~1", "1.1", Match::No),
            ("~1.2.3", "1.3-alpha", Match::No),
            (">=1.2.3-alpha.2", "1.2.3-alpha.10", Match::Yes),
            ("=1.2.3+abc", "1.2.3+def", Match::Yes),
            ("1.2", "1.2.0", Match::Yes),
            ("1.2", "1.2.3", Match::No),
            ("release_foo", "release_foo", Match::Yes),
            (">=1.x", "1.2", Match::Unknown),
            ("(>=1)", "1.2", Match::Unknown),
            ("1 - 2", "1", Match::Unknown),
            (">=1", "unresolved", Match::Unknown),
        ] {
            assert_eq!(
                evaluate(&json!(predicate), version, false),
                expected,
                "{predicate} / {version}"
            );
        }
        assert_eq!(evaluate(&json!(["<1", ">=2"]), "2.1", false), Match::Yes);
        assert_eq!(
            evaluate(&json!({"all":[">1", "<2"]}), "1.5", false),
            Match::Yes
        );
    }
    #[test]
    fn maven_union_open_bounds_and_qualifiers() {
        for (predicate, version, expected) in [
            ("[1.0,2.0)", "1.9", Match::Yes),
            ("[1.0,2.0)", "2", Match::No),
            ("(,1.0],[1.2,)", "1.1", Match::No),
            ("(,1.0],[1.2,)", "9", Match::Yes),
            ("[1.2]", "1.2.0", Match::Yes),
            ("[1.2-rc1,1.2)", "1.2-rc2", Match::Yes),
            ("(1,)", "1", Match::No),
            ("[1,2", "1.5", Match::Unknown),
            ("[1.custom,2)", "1.5", Match::Unknown),
            ("[2,1]", "1.5", Match::Unknown),
            ("[1,2),", "1", Match::Unknown),
        ] {
            assert_eq!(
                evaluate(&json!(predicate), version, true),
                expected,
                "{predicate} / {version}"
            );
        }
    }
}
