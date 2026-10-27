//! Exact logical-depth evidence in compact repeated-value runs.
//! The descriptor still encodes the ordinary DepthEvidence representation.
use super::*;

#[derive(Clone)]
pub(super) struct Depth {
    height: u32,
    limit: u32,
    bounded: bool,
    first: First,
}
#[derive(Clone)]
enum First {
    Empty,
    Runs(Box<[(u32, Option<Span>)]>),
    Full(Box<[Option<Span>]>),
}
impl From<DepthEvidence<Span>> for Depth {
    fn from(value: DepthEvidence<Span>) -> Self {
        let limit = value.limit();
        let mut runs = Vec::new();
        let mut end = 0;
        // Rust slice::chunk_by supplies the maximal equal consecutive runs.
        for run in value.first.chunk_by(|left, right| left == right) {
            end += run.len() as u32;
            runs.push((end, run[0]));
        }
        let first = if runs.len() == 1 && runs[0].1.is_none() {
            First::Empty
        } else if size_of_val(runs.as_slice()) < size_of_val(&*value.first) {
            First::Runs(runs.into_boxed_slice())
        } else {
            First::Full(value.first)
        };
        Self {
            height: value.height,
            limit,
            bounded: value.bounded,
            first,
        }
    }
}
impl Depth {
    pub(super) fn evidence(&self) -> DepthEvidence<Span> {
        let first = match &self.first {
            First::Full(first) => first.clone(),
            First::Empty => vec![None; self.limit as usize + 1].into_boxed_slice(),
            First::Runs(runs) => {
                let mut first = vec![None; self.limit as usize + 1];
                let mut start = 0;
                for &(end, site) in runs {
                    first[start..end as usize].fill(site);
                    start = end as usize;
                }
                first.into_boxed_slice()
            }
        };
        DepthEvidence {
            height: self.height,
            first,
            bounded: self.bounded,
        }
    }
    pub(super) fn retained_bytes(&self) -> usize {
        match &self.first {
            First::Empty => 0,
            First::Runs(runs) => size_of_val(&**runs),
            First::Full(first) => size_of_val(&**first),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compact_depth_preserves_every_request_and_canonical_descriptor() {
        for limit in [0, 1, 7, 32] {
            for bounded in [false, true] {
                for height in [0, 1, 11] {
                    for site_at in [None, Some(0), Some(limit)] {
                        let mut original = DepthEvidence::leaf(limit);
                        original.height = height;
                        original.bounded = bounded;
                        if let Some(at) = site_at {
                            original.first[at as usize] = Some(Span::default());
                        }
                        let compact = Depth::from(original.clone());
                        let restored = compact.evidence();
                        assert_eq!(restored, original);
                        assert_eq!(
                            bincode::serialize(&restored).unwrap(),
                            bincode::serialize(&original).unwrap()
                        );
                        for n in 0..=limit + 1 {
                            assert_eq!(
                                restored.request(n, Span::default()),
                                original.request(n, Span::default())
                            );
                        }
                        assert!(compact.retained_bytes() <= size_of_val(&*original.first));
                        if site_at.is_none() {
                            assert_eq!(compact.retained_bytes(), 0);
                        }
                    }
                }
            }
        }
    }
}
