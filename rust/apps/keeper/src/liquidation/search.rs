//! Bounded adaptive search for profitable, integer-sized liquidations.
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Bound::{Excluded, Included},
};
type Result<T> = std::result::Result<T, String>;
pub fn slices(cap: u64) -> Vec<u64> {
    let mut out = vec![];
    let mut value = cap;
    while value > 0 && out.len() < 16 {
        out.push(value);
        value /= 2;
    }
    if cap > 0 && out.last() != Some(&1) {
        out.push(1);
    }
    out
}

const PROFIT_SEARCH_GRID_SEGMENTS: u64 = 8;
const PROFIT_SEARCH_LOCAL_WIDTH: u64 = 64;
const PROFIT_SEARCH_MAX_PROBES: usize = 256;

fn probe_profit<T, F>(
    amount: u64,
    minimum: u64,
    probes: &mut usize,
    visited: &mut BTreeSet<u64>,
    scores: &mut BTreeMap<u64, u64>,
    evaluate: &mut F,
) -> Result<Option<T>>
where
    F: FnMut(u64) -> Result<Option<(u64, T)>>,
{
    if amount == 0 || *probes >= PROFIT_SEARCH_MAX_PROBES || !visited.insert(amount) {
        return Ok(None);
    }
    *probes += 1;
    let Some((score, payload)) = evaluate(amount)? else {
        return Ok(None);
    };
    scores.insert(amount, score);
    Ok((score >= minimum).then_some(payload))
}

/// Start with the old geometric probes, then use their measured execution
/// profit to refine the best interval. Once the interval is small, scan every
/// integer amount so rounding peaks are not hidden between coarse samples.
pub(super) fn search_profitable<T, F>(cap: u64, minimum: u64, mut evaluate: F) -> Result<Option<T>>
where
    F: FnMut(u64) -> Result<Option<(u64, T)>>,
{
    if cap == 0 {
        return Ok(None);
    }
    let mut probes = 0usize;
    let mut visited = BTreeSet::new();
    let mut scores = BTreeMap::new();
    for amount in slices(cap) {
        if let Some(payload) = probe_profit(
            amount,
            minimum,
            &mut probes,
            &mut visited,
            &mut scores,
            &mut evaluate,
        )? {
            return Ok(Some(payload));
        }
    }
    let Some((&initial_best, _)) = scores
        .iter()
        .max_by_key(|(amount, score)| (**score, **amount))
    else {
        return Ok(None);
    };
    let mut low = visited
        .range(..initial_best)
        .next_back()
        .copied()
        .unwrap_or(1);
    let mut high = visited
        .range((Excluded(initial_best), Included(cap)))
        .next()
        .copied()
        .unwrap_or(cap);

    while high.saturating_sub(low) > PROFIT_SEARCH_LOCAL_WIDTH && probes < PROFIT_SEARCH_MAX_PROBES
    {
        let width = high - low;
        for step in 0..=PROFIT_SEARCH_GRID_SEGMENTS {
            let amount = low
                + u64::try_from(
                    u128::from(width) * u128::from(step) / u128::from(PROFIT_SEARCH_GRID_SEGMENTS),
                )
                .map_err(|_| "profit-search amount overflow")?;
            if let Some(payload) = probe_profit(
                amount,
                minimum,
                &mut probes,
                &mut visited,
                &mut scores,
                &mut evaluate,
            )? {
                return Ok(Some(payload));
            }
        }
        let Some((&best, _)) = scores
            .range(low..=high)
            .max_by_key(|(amount, score)| (**score, **amount))
        else {
            break;
        };
        let next_low = visited.range(low..best).next_back().copied().unwrap_or(low);
        let next_high = visited
            .range((Excluded(best), Included(high)))
            .next()
            .copied()
            .unwrap_or(high);
        if (next_low, next_high) == (low, high) {
            break;
        }
        low = next_low;
        high = next_high;
    }

    if high.saturating_sub(low) <= PROFIT_SEARCH_LOCAL_WIDTH {
        for amount in (low..=high).rev() {
            if let Some(payload) = probe_profit(
                amount,
                minimum,
                &mut probes,
                &mut visited,
                &mut scores,
                &mut evaluate,
            )? {
                return Ok(Some(payload));
            }
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn adaptive_profit_search_finds_non_geometric_rounding_peak() {
        let peaks = [906, 924, 981, 986, 991, 996];
        let mut visited = BTreeSet::new();
        let selected = search_profitable(1_000, 8, |amount| {
            assert!(visited.insert(amount), "amount {amount} probed twice");
            let profit = if peaks.contains(&amount) {
                8
            } else if amount >= 900 {
                7
            } else if amount >= 500 {
                6
            } else if amount >= 250 {
                3
            } else {
                2
            };
            Ok(Some((profit, amount)))
        })
        .unwrap();
        assert_eq!(selected, Some(996));
        assert!(!slices(1_000).contains(&996));
        assert!(visited.len() < PROFIT_SEARCH_MAX_PROBES);
    }
    #[test]
    fn adaptive_profit_search_handles_empty_and_unavailable_domains() {
        let mut calls = 0;
        assert_eq!(
            search_profitable::<u64, _>(0, 1, |_| {
                calls += 1;
                Ok(Some((1, 1)))
            })
            .unwrap(),
            None
        );
        assert_eq!(calls, 0);
        assert_eq!(
            search_profitable::<u64, _>(u64::MAX, 1, |_| {
                calls += 1;
                Ok(None)
            })
            .unwrap(),
            None
        );
        assert!(calls <= 17);
    }
}
