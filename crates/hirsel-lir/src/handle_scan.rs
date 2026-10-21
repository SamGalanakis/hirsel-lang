//! Native projection of the existing holder scan, independent of destruction.
//!
//! An atomic or task handle has a scan even when it has no drop program. Each
//! instruction is eight u64 words. Layout operands are patched to the linked
//! registry by emission; they are never process addresses or local table IDs.
//! Unsupported borrowed/variable-flat representations explicitly yield unknown.

use crate::layout_types::{LayoutId, LayoutKind, LayoutRecord, ScanStep, TagForm};

pub use hirsel_kernels::handle_scan::*;

#[derive(Clone, Debug, Default, Eq, PartialEq, Hash)]
pub struct Program {
    words: Vec<u64>,
    references: Vec<(usize, LayoutId)>,
}

impl Program {
    pub fn lower(record: &LayoutRecord) -> Self {
        let mut out = Self::default();
        if matches!(record.kind, LayoutKind::Sequential { .. }) {
            out.instruction(UNKNOWN, 0);
        } else {
            out.steps(&record.scan);
        }
        out
    }

    pub fn referenced(&self) -> impl Iterator<Item = LayoutId> + '_ {
        self.references.iter().map(|&(_, id)| id)
    }

    /// None means an operand has no executable linked descriptor. Emission
    /// must retain unknown rather than certify an empty capture in that case.
    pub fn linked(&self, index: impl Fn(LayoutId) -> Option<u32>) -> Option<Vec<u64>> {
        let mut words = self.words.clone();
        for &(at, id) in &self.references {
            words[at] = u64::from(index(id)?);
        }
        Some(words)
    }

    fn instruction(&mut self, opcode: u64, offset: u64) -> usize {
        let at = self.words.len();
        self.words.extend([opcode, offset, 0, 0, 0, 0, 0, 0]);
        at
    }

    fn reference(&mut self, at: usize, id: LayoutId) {
        self.references.push((at, id));
    }

    fn steps(&mut self, steps: &[ScanStep]) {
        for step in steps {
            match step {
                ScanStep::Handle { offset } => {
                    self.instruction(HANDLE, *offset);
                }
                ScanStep::Counted { offset }
                | ScanStep::SliceOwner { offset }
                | ScanStep::Environment { offset } => {
                    self.instruction(BLOCK, *offset);
                }
                ScanStep::Root { offset } => {
                    self.instruction(ROOT, *offset);
                }
                ScanStep::Uncounted { .. } => {
                    self.instruction(UNKNOWN, 0);
                }
                ScanStep::Inline { offset, layout } => {
                    let at = self.instruction(INLINE, *offset);
                    self.reference(at + 2, *layout);
                }
                ScanStep::ActiveVariant {
                    tag_offset,
                    form,
                    variants,
                } => {
                    for (ordinal, steps) in variants {
                        let opcode = match form {
                            TagForm::Ordinal => TAG,
                            TagForm::LiveBit { .. } => LIVE_BIT,
                            TagForm::Niche { dataful, .. } if ordinal == dataful => NONNULL,
                            TagForm::Niche { .. } => {
                                if !steps.is_empty() {
                                    self.instruction(UNKNOWN, 0);
                                }
                                continue;
                            }
                        };
                        let at = self.instruction(opcode, *tag_offset);
                        self.words[at + 2] = u64::from(*ordinal);
                        if let TagForm::LiveBit { bit } = form {
                            self.words[at + 4] = u64::from(*bit);
                        }
                        self.steps(steps);
                        self.words[at + 3] = (self.words.len() - at - WORDS) as u64;
                    }
                }
                ScanStep::Sequence {
                    count_offset,
                    offset,
                    stride,
                    element,
                } => {
                    let at = self.instruction(SEQUENCE, *offset);
                    self.words[at + 2] = *count_offset;
                    self.words[at + 3] = *stride;
                    self.reference(at + 4, *element);
                }
                ScanStep::Repeat {
                    count,
                    offset,
                    stride,
                    element,
                } => {
                    let at = self.instruction(REPEAT, *offset);
                    self.words[at + 2] = *count;
                    self.words[at + 3] = *stride;
                    self.reference(at + 4, *element);
                }
                ScanStep::Columns(columns) => {
                    for column in &columns.columns {
                        let at = self.instruction(COLUMN, columns.data_offset);
                        self.words[at + 2] = columns.count_offset;
                        self.words[at + 3] = columns.capacity_offset;
                        self.words[at + 4] = column.stride;
                        self.words[at + 5] = column.prefix;
                        self.reference(at + 6, column.layout);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linking_replaces_every_layout_operand_including_nested_variants() {
        let mut p = Program::default();
        p.steps(&[ScanStep::ActiveVariant {
            tag_offset: 4,
            form: TagForm::Ordinal,
            variants: vec![(
                1,
                vec![ScanStep::Inline {
                    offset: 8,
                    layout: LayoutId(91),
                }],
            )],
        }]);
        assert_eq!(p.referenced().collect::<Vec<_>>(), vec![LayoutId(91)]);
        assert!(p.linked(|_| None).is_none());
        let words = p.linked(|id| (id == LayoutId(91)).then_some(7)).unwrap();
        assert_eq!(&words[..4], &[TAG, 4, 1, WORDS as u64]);
        assert_eq!(&words[WORDS..WORDS + 3], &[INLINE, 8, 7]);
    }

    #[test]
    fn nonresource_handles_are_present_and_borrowed_pointers_are_unknown() {
        let mut p = Program::default();
        p.steps(&[
            ScanStep::Handle { offset: 0 },
            ScanStep::Uncounted { offset: 8 },
        ]);
        let words = p.linked(|_| None).unwrap();
        assert_eq!(words[0], HANDLE);
        assert_eq!(words[WORDS], UNKNOWN);
    }
}
