//! Initialized-field observation, following SER-2's iterative native walk
//! (codec.rs:846,1008): fields in declaration order and only the active arm.
//! Unlike SER-2 this keeps exact floating-point bits and resolves handles to
//! logical object identities. There is no whole-carrier byte slice, user
//! callback, allocation in the run heap, or scheduling point in this decoder.
//! Programs remain private test-admission metadata until compiler integration.

use crate::ordered::footprint::ObjectId;
use crate::ordered::objects::Kind;

const WORK_MAX: usize = 65_536;
const BYTES_MAX: usize = 1 << 20;

#[derive(Clone, Debug)]
pub(super) struct Field {
    pub offset: usize,
    pub node: usize,
}

#[derive(Clone, Debug)]
pub(super) enum Tag {
    Ordinal { offset: usize },
    NonNull { offset: usize, nullary: u32 },
}

#[derive(Clone, Debug)]
pub(super) enum Node {
    Bits(usize),
    Bool,
    Fields {
        extent: usize,
        fields: Vec<Field>,
    },
    Variant {
        extent: usize,
        tag: Tag,
        arms: Vec<Vec<Field>>,
    },
    Handle(Kind),
    /// Missing metadata is never permission to inspect arbitrary bytes.
    Unsupported {
        extent: usize,
    },
}

impl Node {
    fn extent(&self) -> usize {
        match self {
            Self::Bits(bytes) => *bytes,
            Self::Bool => 1,
            Self::Handle(_) => 8,
            Self::Fields { extent, .. }
            | Self::Variant { extent, .. }
            | Self::Unsupported { extent } => *extent,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Error {
    Program,
    Value,
    Unsupported,
    UnresolvedHandle,
    Limit,
}

#[derive(Clone, Debug)]
pub(super) struct Program {
    nodes: Vec<Node>,
    root: usize,
}

impl Program {
    pub(super) fn new(nodes: Vec<Node>, root: usize) -> Result<Self, Error> {
        if nodes.len() > WORK_MAX || root >= nodes.len() {
            return Err(Error::Program);
        }
        for node in &nodes {
            let extent = node.extent();
            let check_fields = |fields: &[Field]| {
                if fields.len() > WORK_MAX {
                    return Err(Error::Program);
                }
                for field in fields {
                    let child = nodes.get(field.node).ok_or(Error::Program)?;
                    if field
                        .offset
                        .checked_add(child.extent())
                        .is_none_or(|end| end > extent)
                    {
                        return Err(Error::Program);
                    }
                }
                Ok(())
            };
            match node {
                Node::Bits(bytes) if !matches!(bytes, 1 | 2 | 4 | 8 | 16) => {
                    return Err(Error::Program);
                }
                Node::Fields { fields, .. } => check_fields(fields)?,
                Node::Variant { tag, arms, .. } => {
                    if arms.is_empty() || arms.len() > WORK_MAX {
                        return Err(Error::Program);
                    }
                    let (offset, size) = match tag {
                        Tag::Ordinal { offset } => (*offset, 4),
                        Tag::NonNull { offset, nullary } => {
                            if arms.len() != 2 || *nullary > 1 {
                                return Err(Error::Program);
                            }
                            (*offset, 8)
                        }
                    };
                    if offset.checked_add(size).is_none_or(|end| end > extent) {
                        return Err(Error::Program);
                    }
                    for arm in arms {
                        check_fields(arm)?;
                    }
                }
                _ => {}
            }
        }
        Ok(Self { nodes, root })
    }

    pub(super) fn extent(&self) -> usize {
        self.nodes[self.root].extent()
    }

    /// # Safety
    /// `value` holds the program's live native value. The program identifies
    /// its actual initialized fields and tags; inactive fields and padding may
    /// be uninitialized. Resolve is read-only, bounded, and cannot yield.
    pub(super) unsafe fn encode(
        &self,
        value: *const u8,
        mut resolve: impl FnMut(Kind, u64) -> Option<ObjectId>,
    ) -> Result<Vec<u8>, Error> {
        if self.extent() > 0 && value.is_null() {
            return Err(Error::Value);
        }
        let mut work = vec![(self.root, value)];
        let mut out = Vec::new();
        let mut visited = 0;
        while let Some((index, at)) = work.pop() {
            visited += 1;
            if visited > WORK_MAX {
                return Err(Error::Limit);
            }
            let fields = match &self.nodes[index] {
                Node::Bits(bytes) => {
                    // Read exactly the scalar, never its containing carrier.
                    let bits = unsafe {
                        match bytes {
                            1 => u128::from(at.read()),
                            2 => u128::from(at.cast::<u16>().read_unaligned()),
                            4 => u128::from(at.cast::<u32>().read_unaligned()),
                            8 => u128::from(at.cast::<u64>().read_unaligned()),
                            16 => at.cast::<u128>().read_unaligned(),
                            _ => unreachable!("validated scalar width"),
                        }
                    };
                    out.extend_from_slice(&bits.to_le_bytes()[..*bytes]);
                    continue;
                }
                Node::Bool => {
                    let value = unsafe { at.read() };
                    if value > 1 {
                        return Err(Error::Value);
                    }
                    out.push(value);
                    continue;
                }
                Node::Handle(kind) => {
                    let word = unsafe { at.cast::<u64>().read_unaligned() };
                    let object = resolve(*kind, word).ok_or(Error::UnresolvedHandle)?;
                    if object.creator.len() > WORK_MAX {
                        return Err(Error::Limit);
                    }
                    out.extend_from_slice(&kind.code().to_le_bytes());
                    out.extend_from_slice(&(object.creator.len() as u64).to_le_bytes());
                    for part in object.creator {
                        out.extend_from_slice(&part.to_le_bytes());
                    }
                    out.extend_from_slice(&object.ordinal.to_le_bytes());
                    if out.len() > BYTES_MAX {
                        return Err(Error::Limit);
                    }
                    continue;
                }
                Node::Fields { fields, .. } => fields,
                Node::Variant { tag, arms, .. } => {
                    let ordinal = unsafe {
                        match tag {
                            Tag::Ordinal { offset } => {
                                at.add(*offset).cast::<u32>().read_unaligned()
                            }
                            Tag::NonNull { offset, nullary } => {
                                if at.add(*offset).cast::<u64>().read_unaligned() == 0 {
                                    *nullary
                                } else {
                                    1 - *nullary
                                }
                            }
                        }
                    };
                    let arm = arms.get(ordinal as usize).ok_or(Error::Value)?;
                    out.extend_from_slice(&ordinal.to_le_bytes());
                    arm
                }
                Node::Unsupported { .. } => return Err(Error::Unsupported),
            };
            if work.len() + fields.len() > WORK_MAX {
                return Err(Error::Limit);
            }
            for field in fields.iter().rev() {
                work.push((field.node, unsafe { at.add(field.offset) }));
            }
        }
        if out.len() > BYTES_MAX {
            return Err(Error::Limit);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::MaybeUninit;

    #[test]
    fn active_fields_skip_uninitialized_record_padding_and_inactive_arms() {
        let program = Program::new(
            vec![
                Node::Variant {
                    extent: 24,
                    tag: Tag::Ordinal { offset: 0 },
                    arms: vec![vec![], vec![Field { offset: 8, node: 1 }]],
                },
                Node::Fields {
                    extent: 16,
                    fields: vec![Field { offset: 0, node: 2 }, Field { offset: 8, node: 3 }],
                },
                Node::Bool,
                Node::Bits(8),
            ],
            0,
        )
        .unwrap();
        for active in [false, true] {
            let mut value = MaybeUninit::<[u64; 3]>::uninit();
            let at = value.as_mut_ptr().cast::<u8>();
            unsafe {
                at.cast::<u32>().write(u32::from(active));
                if active {
                    at.add(8).write(1);
                    at.add(16).cast::<u64>().write(42);
                }
                let encoded = program.encode(at, |_, _| panic!("no handles")).unwrap();
                assert_eq!(
                    encoded,
                    if active {
                        [1u32.to_le_bytes().as_slice(), &[1], &42u64.to_le_bytes()].concat()
                    } else {
                        0u32.to_le_bytes().to_vec()
                    }
                );
            }
        }
    }

    #[test]
    fn handle_words_do_not_escape_and_inactive_niche_payloads_are_not_read() {
        let program = Program::new(
            vec![
                Node::Variant {
                    extent: 16,
                    tag: Tag::NonNull {
                        offset: 0,
                        nullary: 0,
                    },
                    arms: vec![
                        vec![],
                        vec![Field { offset: 0, node: 1 }, Field { offset: 8, node: 2 }],
                    ],
                },
                Node::Handle(Kind::Sender),
                Node::Bits(8),
            ],
            0,
        )
        .unwrap();
        let mut reference = None;
        for word in [7u64, 999] {
            let value = [word, 42];
            let bytes = unsafe {
                program.encode(value.as_ptr().cast(), |kind, seen| {
                    assert_eq!((kind, seen), (Kind::Sender, word));
                    Some(ObjectId {
                        creator: vec![1, 2],
                        ordinal: 5,
                    })
                })
            }
            .unwrap();
            if let Some(reference) = &reference {
                assert_eq!(&bytes, reference);
            } else {
                reference = Some(bytes);
            }
        }
        let mut none = MaybeUninit::<[u64; 2]>::uninit();
        unsafe {
            none.as_mut_ptr().cast::<u64>().write(0);
            assert_eq!(
                program.encode(none.as_ptr().cast(), |_, _| panic!("inactive handle")),
                Ok(0u32.to_le_bytes().to_vec())
            );
        }
        assert_eq!(
            unsafe { program.encode([7u64, 42].as_ptr().cast(), |_, _| None) },
            Err(Error::UnresolvedHandle)
        );
    }

    #[test]
    fn invalid_metadata_values_and_recursive_work_refuse_the_entire_observation() {
        assert!(Program::new(vec![Node::Bits(3)], 0).is_err());
        assert!(
            Program::new(
                vec![
                    Node::Fields {
                        extent: 8,
                        fields: vec![Field {
                            offset: usize::MAX,
                            node: 1
                        }]
                    },
                    Node::Bits(8)
                ],
                0
            )
            .is_err()
        );
        let unsupported = Program::new(vec![Node::Unsupported { extent: 8 }], 0).unwrap();
        assert_eq!(
            unsafe {
                unsupported.encode(MaybeUninit::<u64>::uninit().as_ptr().cast(), |_, _| None)
            },
            Err(Error::Unsupported)
        );
        let recursive = Program::new(
            vec![Node::Fields {
                extent: 0,
                fields: vec![Field { offset: 0, node: 0 }],
            }],
            0,
        )
        .unwrap();
        assert_eq!(
            unsafe { recursive.encode(std::ptr::NonNull::<u8>::dangling().as_ptr(), |_, _| None) },
            Err(Error::Limit)
        );
        let boolean = Program::new(vec![Node::Bool], 0).unwrap();
        assert_eq!(
            unsafe { boolean.encode(&2, |_, _| None) },
            Err(Error::Value)
        );
    }
}
